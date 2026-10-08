//! The virtual-model **registry** and the **routing step**: who is registered, and what physical
//! model each request goes to.
//!
//! [`crate::virtual_models`] is the catalog half — the entry a virtual model appears as, the
//! "must be routed" error, and the provider decorator that lists virtual rows beside physical
//! ones. This module is the half above it: the router callback surface
//! ([`ModelRouter`], [`ModelRouteRequest`], [`ModelRoute`]), the [`VirtualModelRegistry`] that
//! holds `{ model, route }` per `(provider, id)`, and [`VirtualModelRegistry::resolve_model`],
//! which asks a router for one request's physical model and **validates the answer** before anyone
//! streams it.
//!
//! Ported from pi `packages/coding-agent/src/core/model-runtime.ts`, read at **v1.0.4** through git
//! objects — `registerVirtualModel` / `unregisterVirtualModel` (`:947-982`), `resolveModel`
//! (`:994-1024`) and `getPhysicalModel` (`:1026-1030`) — plus the route types and
//! `findLatestResponse` from `packages/coding-agent/src/core/virtual-models.ts` (`:43-118`).
//!
//! # Why the registry lives in `cyrup-provider` and not in the session crate
//!
//! Upstream's registry is a member of `ModelRuntime`, which IS pi's `Models` collection, so it
//! reads the catalog and the auth snapshot off `this`. cyrup has no long-lived `Models` inside a
//! session: the catalog is recomposed on demand and provider auth is a predicate rebuilt per read.
//! Both of pi's reads are therefore **injected** here, as [`VirtualModelCatalog`], which keeps this
//! type free of any session dependency. That matters twice over:
//!
//! - `cyrup-ext` must name [`ModelRouter`] and [`VirtualModelDefinition`] to offer
//!   `register_virtual_model`, and it depends on `cyrup-provider`, not on the session service.
//! - The session builder needs a registry **before** it restores the branch selection, which is
//!   long before any catalog exists (pi drains its pending registrations ahead of restore —
//!   `agent-session-services.ts:182-194`).
//!
//! # What this module does NOT do
//!
//! It does not call routers. Nothing here is wired into a request path: deriving `reason`,
//! `previous`'s transcript, the stashed `failed` response and the branch-stored `state`, and
//! persisting the state a router returns, all belong to the agent-loop seam that owns
//! `prepare_request`. This module only makes that seam a few function calls wide — see
//! [`VirtualModelRegistry::resolve_model`]'s "Calling this from the agent loop".

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde_json::Value;

use crate::collection::clamp_thinking_level;
use crate::model::Model;
use crate::virtual_models::{VirtualModelSpec, create_virtual_model, is_virtual_model};
use cyrup_core::{
    AssistantMessage, CancelToken, Message, ModelId, ModelThinkingLevel, ProviderId, StopReason,
};

// ------------------------------------------------------------------ route types ----

/// Why a request is being routed — pi `ModelRouteReason` (`virtual-models.ts:43-50`).
///
/// pi's own definitions, verbatim:
///
/// - `user`: first request after a message the user wrote (prompt, steering, or follow-up)
/// - `continuation`: any other request in the agent loop, e.g. after tool results or extension
///   messages
/// - `retry`: automatic retry after a failed request, including after compaction for a context
///   overflow
/// - `direct`: a request outside the agent loop, e.g. a compaction summary or an extension call
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelRouteReason {
    User,
    Continuation,
    Retry,
    Direct,
}

impl ModelRouteReason {
    /// pi's own string form, which is what a router matching on the reason compares against and
    /// what upstream's tests assert (`test/suite/virtual-models.test.ts`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Continuation => "continuation",
            Self::Retry => "retry",
            Self::Direct => "direct",
        }
    }
}

impl std::fmt::Display for ModelRouteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A physical model and the thinking level that went with it — pi's
/// `{ model: Model<Api>; thinkingLevel?: ModelThinkingLevel }` (`virtual-models.ts:58`).
///
/// Used for [`ModelRouteRequest::previous`]. The level is an [`Option`] because pi's is optional:
/// it is read off the assistant message, which carries it only for responses the agent loop
/// produced.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutedModel {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// The failed request a retry is retrying — pi's
/// `{ model: Model<Api>; thinkingLevel?: ModelThinkingLevel; message: AssistantMessage }`
/// (`virtual-models.ts:63`).
///
/// `message` carries the failure's `stop_reason` and `error_message`, which is how a router decides
/// whether to re-route (an overload) or stay put (a tool-format error).
#[derive(Clone, Debug, PartialEq)]
pub struct FailedRequest {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub message: AssistantMessage,
}

/// Everything a router is told about one request — pi `ModelRouteRequest`
/// (`virtual-models.ts:52-70`).
///
/// Borrowed rather than owned: pi hands the router the live `messages` array, and a transcript is
/// the largest thing in a session. A router that needs to keep something clones it itself.
pub struct ModelRouteRequest<'a> {
    /// The selected VIRTUAL model (pi `model`), not the one being routed to.
    pub model: &'a Model,
    /// The selected thinking level. **Its meaning is up to the router** (pi's own words, `:56`) —
    /// a router may read it as a budget hint rather than as a level to pass on.
    pub thinking_level: ModelThinkingLevel,
    pub reason: ModelRouteReason,
    /// Physical model and thinking level of the latest successful response in `messages`
    /// (pi `previous?`, `:58`). See [`find_latest_response`].
    pub previous: Option<RoutedModel>,
    /// For [`ModelRouteReason::Retry`]: the failed request, which `messages` no longer contains
    /// (pi `failed?`, `:63`). **Absent when the router itself failed** — a failed routing attempt
    /// names the virtual model, so there is no physical request to report.
    pub failed: Option<FailedRequest>,
    /// Router state last returned on this session branch (pi `state?`, `:65`). `None` before the
    /// first state and for every [`ModelRouteReason::Direct`] request.
    pub state: Option<&'a Value>,
    /// The conversation for this request, **including system messages** (pi `messages`, `:67`).
    pub messages: &'a [Message],
    /// pi's `signal?: AbortSignal`.
    pub cancel: CancelToken,
    /// The session's model catalog — pi `ctx.modelRegistry`, which every router upstream uses to
    /// turn the `(provider, id)` it decided on into the `Model` it must return
    /// (`examples/extensions/jev-router.ts:41`: `const model = ctx.modelRegistry.find(PROVIDER,
    /// id)`), and which `docs/virtual-models.md` points routers at.
    ///
    /// `[CYRUP-DELTA]` POSITION, not presence. Upstream passes it as a SECOND parameter of the
    /// extension-tier `route(request, ctx)` (`extensions/types.ts:1886-1889` @v1.0.4) and curries
    /// it in per request at the registration seam — `route: (request) => model.route(request,
    /// runtime.createContext())` (`extensions/loader.ts:500-508`), whose own comment is *"Routing
    /// runs after the runner binds, so the context is created per request"*. So upstream's
    /// registry-level `VirtualModelDefinition.route` takes one argument, exactly as
    /// [`ModelRouter::route`] does, and the context is nonetheless a PER-REQUEST injection.
    ///
    /// Rust has no cheap way to curry a borrowed trait object into a `dyn` method, and the catalog
    /// is already live at the one call site ([`VirtualModelRegistry::resolve_model`] takes it as an
    /// argument), so the injection rides on the request instead of on a second parameter. Same
    /// value, same per-request lifetime, same `&dyn` so no router can retain it.
    ///
    /// Only the two catalog reads [`VirtualModelCatalog`] declares are offered, not the whole
    /// extension context: those are what upstream's routers and the docs page actually use, and a
    /// router that could reach the rest of the host from inside the routing step would be a
    /// capability upstream does not grant it either.
    pub catalog: &'a dyn VirtualModelCatalog,
}

/// Hand-written because [`VirtualModelCatalog`] is not `Debug` — it is a trait every host layer
/// implements over its own live state, and requiring `Debug` of it would force every implementor
/// to render a catalog of ~1100 rows. The field prints as an opaque marker; everything else is the
/// derive's own output, field for field and in declaration order.
impl std::fmt::Debug for ModelRouteRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouteRequest")
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("reason", &self.reason)
            .field("previous", &self.previous)
            .field("failed", &self.failed)
            .field("state", &self.state)
            .field("messages", &self.messages)
            .field("cancel", &self.cancel)
            .field("catalog", &"<dyn VirtualModelCatalog>")
            .finish()
    }
}

/// A physical model and thinking level for one request — pi `ModelRoute`
/// (`virtual-models.ts:72-82`).
///
/// This is both what a [`ModelRouter`] returns and what
/// [`VirtualModelRegistry::resolve_model`] answers, exactly as upstream uses one type for both.
/// On the way out of `resolve_model` the `model` has been re-resolved against the catalog and the
/// level clamped to it, so a caller never has to re-check either.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRoute {
    pub model: Model,
    pub thinking_level: ModelThinkingLevel,
    /// New router state, stored on the session branch unless it is `request.state` itself. Return
    /// `request.state` or `None` to keep the current state. Must be JSON-serializable (it is a
    /// [`Value`], so it is). **Ignored for [`ModelRouteReason::Direct`] requests.**
    pub state: Option<Value>,
}

/// A router's own failure — pi's `route()` throwing.
///
/// Unlike `classify` and `generate_images`, which upstream documents as never rejecting, a route
/// genuinely can fail, and upstream's contract for that is explicit: *"If `route()` throws, or
/// returns a virtual model or a model without credentials, the request ends with an error
/// response"* (`docs/virtual-models.md`). So the fallibility is in the signature rather than
/// smuggled into an errored envelope, and the message reaches the user unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ModelRouteError(pub String);

impl ModelRouteError {
    /// Build one from anything printable, for routers that have a richer error of their own.
    pub fn new(message: impl std::fmt::Display) -> Self {
        Self(message.to_string())
    }
}

/// Picks the physical model and thinking level for one request — pi
/// `VirtualModelDefinition.route` (`virtual-models.ts:101`).
///
/// Async because upstream's is (`ModelRoute | Promise<ModelRoute>`) and because real routers do
/// I/O: upstream's worked example classifies the prompt through the model registry before
/// deciding. The returned model need only name a physical catalog model —
/// [`VirtualModelRegistry::resolve_model`] re-resolves `provider`/`id` against the catalog and
/// rejects the answer if it is virtual, unknown, or has no credentials.
#[async_trait::async_trait]
pub trait ModelRouter: Send + Sync {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError>;
}

/// A virtual model as it is registered — pi `VirtualModelDefinition`
/// (`virtual-models.ts:84-102`).
///
/// [`VirtualModelSpec`] is pi's `Omit<VirtualModelDefinition, "route">`, which the catalog layer
/// owns because it is all [`create_virtual_model`] needs; this composes the router back onto it,
/// which is pi's whole type. Splitting it that way is what keeps `cyrup-provider`'s catalog half
/// free of a callback and lets a caller build the catalog entry without a router at all.
#[derive(Clone)]
pub struct VirtualModelDefinition {
    pub spec: VirtualModelSpec,
    pub router: Arc<dyn ModelRouter>,
}

impl VirtualModelDefinition {
    /// `{ spec, router }`, spelled for the common call.
    pub fn new(spec: VirtualModelSpec, router: Arc<dyn ModelRouter>) -> Self {
        Self { spec, router }
    }
}

impl std::fmt::Debug for VirtualModelDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A router is a closure or an extension callback: there is nothing printable behind it.
        f.debug_struct("VirtualModelDefinition")
            .field("spec", &self.spec)
            .field("router", &"<dyn ModelRouter>")
            .finish()
    }
}

// ------------------------------------------------------------- latest response ----

/// The latest **successful** response in a transcript — pi `findLatestResponse`
/// (`virtual-models.ts:109-118`).
///
/// In upstream's own words: *"Its model is physical: failed or aborted requests, including failed
/// routing, are skipped."* That is what makes it safe to feed
/// [`ModelRouteRequest::previous`] — a routing failure leaves the VIRTUAL model on its assistant
/// message, and skipping `Error` is what keeps that message from being reported as a physical
/// previous request.
///
/// [`StopReason::Deferred`] is **not** skipped, because upstream skips exactly `"error"` and
/// `"aborted"` and nothing else. cyrup's `Deferred` has no upstream counterpart in this walk, and a
/// deferred turn does name a real physical model, so following upstream literally is also the
/// defensible answer; it is called out here so the choice is visible rather than inherited.
#[must_use]
pub fn find_latest_response(messages: &[Message]) -> Option<&AssistantMessage> {
    messages.iter().rev().find_map(|message| match message {
        Message::Assistant(a)
            if a.stop_reason != StopReason::Error && a.stop_reason != StopReason::Aborted =>
        {
            Some(a)
        }
        _ => None,
    })
}

/// The Pi thinking level the agent loop requested for a response — pi
/// `AssistantMessage.thinkingLevel` (`packages/ai/src/types.ts:559` @v1.0.4: *"Pi thinking level
/// the agent loop requested for this response. Absent outside the agent loop and for legacy
/// responses."*).
///
/// # The routing step's only thinking-level read, and deliberately one function
///
/// CORRECTED 2026-10-07. This heading used to read *"This is the one unported field the routing
/// step reads"*, which PROV-127 made false: `AssistantMessage::thinking_level` IS ported — the
/// field is declared at `crates/cyrup-core/src/message/assistant.rs:104`, has its `thinkingLevel`
/// slot in the hand-written serializer at `:230`, and is stamped in exactly one place
/// (`Settled::with_thinking_level`, `crates/cyrup-agent/src/agent/run/assistant_stream.rs:62`,
/// called from `crates/cyrup-agent/src/agent/run/stream.rs:172`). The heading contradicted its own
/// body, which already described a ported field. What survives of the old claim is the reason this
/// is a function and not a field access: the routing step must read the canonical rung and never
/// the provider-native string.
///
/// cyrup's [`AssistantMessage`] carries both pi fields, and they are different things:
/// `provider_thinking_level` is the provider-NATIVE effort string the adapter actually sent, while
/// `thinking_level` is the canonical rung the agent loop ASKED for. This reads the second, which is
/// the one pi reads — it is stamped in exactly one place, on the settled result inside the loop
/// (`packages/agent/src/agent-loop.ts:408-409`), so under a virtual selection it records the
/// ROUTED level and a sticky router holds the level the turn was answered at.
///
/// `None` for a message produced outside the agent loop and for a session file written before the
/// field existed, which is upstream's own contract for it ("Absent outside the agent loop and for
/// legacy responses", `packages/ai/src/types.ts:558`). Every router upstream ships therefore keeps
/// its `?? <default>` arm, and so must any router written here.
#[must_use]
pub fn message_thinking_level(message: &AssistantMessage) -> Option<ModelThinkingLevel> {
    message.thinking_level
}

// ------------------------------------------------------------------- catalog seam ----

/// The two catalog reads the registry needs, which upstream takes off `this`.
///
/// pi's `ModelRuntime` IS the `Models` collection, so `registerVirtualModel` reads
/// `this.models.getModel(...)` and `resolveModel` reads `this.hasConfiguredAuth(...)` directly. In
/// cyrup both live above this crate, so they arrive as a trait object and the registry stays
/// session-free. Pass [`NoCatalog`] when there is genuinely no catalog yet — the pre-restore
/// registration drain is exactly that situation.
pub trait VirtualModelCatalog: Send + Sync {
    /// pi `this.models.getModel(providerId, modelId)` — a chat catalog model of any kind,
    /// INCLUDING a virtual one, because the conflict check has to tell them apart.
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model>;

    /// pi `this.hasConfiguredAuth(providerId)` — whether that provider has credentials. The
    /// routing step rejects a route to a provider for which this is `false`.
    fn has_configured_auth(&self, provider: &str) -> bool;

    /// Whether anything other than virtual models defines this provider id.
    ///
    /// pi gets this from `recomposeProvider`'s return value, which is documented as *"the provider
    /// without virtual models, or undefined when only virtual models define it"*
    /// (`model-runtime.ts:159-160`). It decides one thing: whether registering under this id must
    /// also mark the id configured, because a provider of only virtual models needs no credentials
    /// (`:962-968`).
    fn has_physical_provider(&self, provider: &str) -> bool;

    /// Every chat model the catalog lists, virtual rows included — pi
    /// `ctx.modelRegistry.getModels()` (`model-registry.ts:134-140`, delegating to
    /// `ModelRuntime.getModels`).
    ///
    /// The routing step itself never calls this. It exists because the catalog reaches the ROUTER
    /// through [`ModelRouteRequest::catalog`], and a router that picks its target by a property
    /// rather than by a hard-coded id (the largest context window, the cheapest, the only one of a
    /// family that has credentials) needs the listing and not just the lookup. Upstream's own
    /// `jev-router.ts` needs only `find`, but `ctx.modelRegistry` offers both and the docs page
    /// points routers at the whole registry.
    fn models(&self) -> Vec<Model>;

    /// A catalog chat model that is **not** virtual — pi `getPhysicalModel`
    /// (`model-runtime.ts:1026-1030`).
    ///
    /// Provided, not required: it is `get_model` plus the virtual test, and every caller in the
    /// routing step wants this form rather than the raw lookup.
    fn physical_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.get_model(provider, model_id)
            .filter(|m| !is_virtual_model(m))
    }
}

/// A [`VirtualModelCatalog`] that knows nothing: no models, no credentials, no providers.
///
/// Registering against it can only fail on an empty provider or id, and every id it is asked about
/// reads as virtual-only — which is the right answer before any provider has been composed, and is
/// why pi's own pre-restore drain can mark a virtual-only provider configured without consulting a
/// catalog either.
///
/// It is **not** usable for [`VirtualModelRegistry::resolve_model`]: every route would be rejected
/// as "not a physical model". That is a correct answer, not a trap — nothing can be routed before
/// there is a catalog to route into.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCatalog;

impl VirtualModelCatalog for NoCatalog {
    fn get_model(&self, _provider: &str, _model_id: &str) -> Option<Model> {
        None
    }
    fn models(&self) -> Vec<Model> {
        Vec::new()
    }
    fn has_configured_auth(&self, _provider: &str) -> bool {
        false
    }
    fn has_physical_provider(&self, _provider: &str) -> bool {
        false
    }
}

// ------------------------------------------------------------------------ errors ----

/// Everything registration and routing can refuse, with **pi's exact message text**.
///
/// Each variant's `Display` reproduces one upstream `throw new Error(...)` byte for byte, because
/// these strings reach the user: a routing failure ends the run with an error response whose
/// `error_message` is this text, and upstream's own tests assert on substrings of it
/// (`test/virtual-models.test.ts:191`, `:205`, `:260`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VirtualModelError {
    /// pi `"Virtual model provider and id must not be empty."` (`model-runtime.ts:956`).
    #[error("Virtual model provider and id must not be empty.")]
    EmptyProviderOrId,

    /// pi ```Virtual model ${providerId}/${id} conflicts with a physical model.` ``
    /// (`model-runtime.ts:959`).
    #[error("Virtual model {provider}/{id} conflicts with a physical model.")]
    PhysicalConflict { provider: ProviderId, id: ModelId },

    /// pi ```${name} is not registered.` `` (`model-runtime.ts:998`).
    #[error("Virtual model {provider}/{id} is not registered.")]
    NotRegistered { provider: ProviderId, id: ModelId },

    /// The router's own failure — pi's `route()` throwing, whose message propagates unchanged.
    #[error("{0}")]
    Route(#[from] ModelRouteError),

    /// pi ```${routed}, which is not a physical model.` `` (`model-runtime.ts:1021`). Covers both
    /// an unknown model and a route to another VIRTUAL model: upstream uses one message for both,
    /// and its own test drives both through it (`test/virtual-models.test.ts:198-207`).
    #[error(
        "Virtual model {provider}/{id} routed to {target_provider}/{target_id}, which is not a physical model."
    )]
    NotPhysical {
        provider: ProviderId,
        id: ModelId,
        target_provider: ProviderId,
        target_id: ModelId,
    },

    /// pi ```${routed}, which has no credentials.` `` (`model-runtime.ts:1022`).
    #[error(
        "Virtual model {provider}/{id} routed to {target_provider}/{target_id}, which has no credentials."
    )]
    NoCredentials {
        provider: ProviderId,
        id: ModelId,
        target_provider: ProviderId,
        target_id: ModelId,
    },
}

// ---------------------------------------------------------------------- listener ----

/// Told when the set of registered virtual models changed, so the catalog that embeds them can be
/// rebuilt.
///
/// This is pi's tail of `registerVirtualModel` / `unregisterVirtualModel`, which the registry
/// itself cannot perform here because all three steps live above this crate:
///
/// 1. `recomposeProvider(providerId)` — re-wrap that provider id. cyrup has no decorated
///    provider to re-wrap: the merge is [`VirtualModelRegistry::apply_to_catalog`] over the flat
///    composed catalog, so this step is "rebuild the composed catalog", which is step 2's cache
///    invalidation and nothing more. See [`crate::unrouted_message`] for why the decorator port
///    is gone.
/// 2. `updateModelSnapshot()` — invalidate whatever caches the composed catalog.
///    [`VirtualModelRegistry::generation`] is the cache key for this; a snapshot that does not
///    include it serves a stale catalog and the new virtual model never appears.
/// 3. `void this.refresh({ allowNetwork: false })` (`model-runtime.ts:973`, `:982`) — the
///    **cache-only** whole-registry refresh, which is the behaviour `SEAM-144` names. It is
///    cache-only and whole-registry, not a per-id network restore.
///
/// Implementations must not block: upstream's third step is a floating promise (`void`), so the
/// mutator returns without waiting for it.
pub trait VirtualModelListener: Send + Sync {
    fn virtual_models_changed(&self, provider: &ProviderId);
}

// ---------------------------------------------------------------------- registry ----

/// One registered virtual model — pi `RegisteredVirtualModel { model, route }`
/// (`model-runtime.ts:94-97`), plus the `(provider, id)` key pi keeps in the two map levels.
#[derive(Clone)]
struct RegisteredVirtualModel {
    provider: ProviderId,
    id: ModelId,
    model: Model,
    router: Arc<dyn ModelRouter>,
}

#[derive(Default)]
struct RegistryState {
    /// Every registration, **in registration order** — see [`VirtualModelRegistry`]'s docs for why
    /// this is a `Vec` and not a map.
    entries: Vec<RegisteredVirtualModel>,
    /// Provider ids nothing but virtual models defines, which therefore count as configured
    /// without credentials. pi's provisional `auth.set(providerId, { type: "api_key", source:
    /// "virtual" })` (`model-runtime.ts:962-968`).
    ///
    /// A `Vec` rather than a set because [`ProviderId`] is not `Ord` and because pi's
    /// `configuredProviders` is a JS `Set`, i.e. insertion-ordered; membership is tested linearly
    /// over a handful of ids.
    virtual_only: Vec<ProviderId>,
}

/// Virtual models by `(provider, id)`, with their routers — pi's
/// `virtualModels: Map<string, Map<string, RegisteredVirtualModel>>` (`model-runtime.ts:179`).
///
/// # Ordering is observable, so this is a `Vec` and not a `BTreeMap`
///
/// pi's two levels are both JS `Map`s, i.e. **insertion-ordered**, and that order reaches the
/// user: upstream pins `getModels("faux") == ["small", "large", "auto", "fast"]` and
/// `getModels("router") == ["auto", "second"]` (`test/virtual-models.test.ts:174-175`), which is
/// physical rows first and then virtual rows in REGISTRATION order. A `BTreeMap<String, _>` sorts
/// by model id and would match those two lists only by coincidence. A flat `Vec` in registration
/// order is faithful by construction, and re-registering replaces **in place** so a replacement
/// does not move a row to the end — which is `Map.set`'s behaviour on an existing key.
///
/// The registry is cheap to read and rarely written (a handful of entries, written at extension
/// load), so a `std::sync::RwLock` is the right lock and no read ever spans an `await`:
/// [`Self::resolve_model`] clones the router [`Arc`] out and releases the guard before calling it.
///
/// # Shared, not cloned
///
/// Hold it as `Arc<VirtualModelRegistry>`. Every mutator takes `&self`, so the extension seam, the
/// session builder and the routing step share one instance; a clone would silently fork the set.
#[derive(Default)]
pub struct VirtualModelRegistry {
    state: RwLock<RegistryState>,
    /// Bumped on every mutation, so a composed-catalog cache can key on it.
    generation: AtomicU64,
    listener: RwLock<Option<Arc<dyn VirtualModelListener>>>,
}

impl std::fmt::Debug for VirtualModelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids = self
            .state
            .read()
            .ok()
            .map(|s| {
                s.entries
                    .iter()
                    .map(|e| format!("{}/{}", e.provider, e.id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        f.debug_struct("VirtualModelRegistry")
            .field("generation", &self.generation())
            .field("models", &ids)
            .finish()
    }
}

impl VirtualModelRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A monotonic counter bumped by every [`Self::register`] and every [`Self::unregister`] that
    /// removed something.
    ///
    /// This is the cache key for a composed catalog. pi has no counterpart because it mutates the
    /// `Models` collection in place; cyrup recomposes behind a cache, and a cache whose key does
    /// not include this serves the pre-registration catalog forever. The failure mode is quiet — a
    /// registration simply never appears — and it cannot be detected by a test that registers
    /// before the first catalog read.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Whether nothing at all is registered. A session with no virtual models must behave exactly
    /// as it did before this feature existed, and this is the cheap test for taking that path.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read(|s| s.entries.is_empty())
    }

    /// Install the listener that rebuilds the catalog after a mutation — see
    /// [`VirtualModelListener`]. Replaces any previous one.
    pub fn set_listener(&self, listener: Arc<dyn VirtualModelListener>) {
        if let Ok(mut slot) = self.listener.write() {
            *slot = Some(listener);
        }
    }

    /// Register a virtual model under `definition.spec.provider` — pi `registerVirtualModel`
    /// (`model-runtime.ts:947-974`).
    ///
    /// Upstream's own summary: *"Register a virtual model under `definition.provider`, which may
    /// also list physical models or several virtual models. Re-registering the same provider and id
    /// replaces the virtual model. Throws when the id belongs to a physical model of that
    /// provider."*
    ///
    /// The three checks run in pi's order, and the two refusals carry pi's exact text
    /// ([`VirtualModelError::EmptyProviderOrId`], [`VirtualModelError::PhysicalConflict`]). A
    /// conflict is tested against `catalog.get_model`, NOT against
    /// [`VirtualModelCatalog::physical_model`]: an id that already names a VIRTUAL model is a
    /// replacement, not a conflict.
    ///
    /// On success it also performs pi's virtual-only marking (`:962-968`) and notifies the listener.
    ///
    /// # Errors
    ///
    /// [`VirtualModelError::EmptyProviderOrId`] when either id trims to nothing, and
    /// [`VirtualModelError::PhysicalConflict`] when the id already names a physical model of that
    /// provider. Nothing is registered in either case.
    pub fn register(
        &self,
        definition: VirtualModelDefinition,
        catalog: &dyn VirtualModelCatalog,
    ) -> Result<(), VirtualModelError> {
        let provider = definition.spec.provider.clone();
        let id = definition.spec.id.clone();
        // `if (!providerId.trim() || !id.trim()) throw ...` (`:956`).
        if provider.as_str().trim().is_empty() || id.as_str().trim().is_empty() {
            return Err(VirtualModelError::EmptyProviderOrId);
        }
        // `const existing = this.models.getModel(providerId, id); if (existing &&
        // !isVirtualModel(existing)) throw ...` (`:957-960`).
        if catalog
            .get_model(provider.as_str(), id.as_str())
            .is_some_and(|m| !is_virtual_model(&m))
        {
            return Err(VirtualModelError::PhysicalConflict { provider, id });
        }
        let model = create_virtual_model(&definition.spec);
        let Ok(mut state) = self.state.write() else {
            // A poisoned registry would mean a router panicked while the guard was held, which
            // cannot happen: no router is called under this lock.
            return Ok(());
        };
        // `models.set(id, { model, route })` — replace IN PLACE, because `Map.set` on an existing
        // key keeps its insertion position and the position is observable (see the type docs).
        match state
            .entries
            .iter_mut()
            .find(|e| e.provider == provider && e.id == id)
        {
            Some(slot) => {
                slot.model = model;
                slot.router = definition.router;
            }
            None => state.entries.push(RegisteredVirtualModel {
                provider: provider.clone(),
                id,
                model,
                router: definition.router,
            }),
        }
        // `if (!this.recomposeProvider(providerId) && !this.snapshot.configuredProviders.has(
        // providerId))` mark it configured — "A provider of only virtual models needs no
        // credentials. Mark it configured now: session restore checks auth before the refresh
        // below lands." (`:962-968`).
        if !catalog.has_physical_provider(provider.as_str())
            && !catalog.has_configured_auth(provider.as_str())
            && !state.virtual_only.contains(&provider)
        {
            state.virtual_only.push(provider.clone());
        }
        drop(state);
        self.bump_and_notify(&provider);
        Ok(())
    }

    /// Remove one virtual model — pi `unregisterVirtualModel` (`model-runtime.ts:976-982`).
    ///
    /// Returns whether anything was removed, which is pi's `if (!models?.delete(id)) return;`: an
    /// absent `(provider, id)` is a **no-op**, with no recompose and no refresh. When the
    /// provider's last virtual model goes, the provider's entry goes with it (`:978`) — and so does
    /// its virtual-only marking, since nothing lists it any more.
    ///
    /// It deliberately does NOT remove a provider's virtual models when the provider itself is
    /// unregistered: upstream's `unregisterProvider` leaves them alone (`:966-972`), and the doc
    /// page says so outright — *"`pi.unregisterVirtualModel(provider, id)` removes it;
    /// `pi.unregisterProvider()` does not"*.
    pub fn unregister(&self, provider: &str, id: &str) -> bool {
        let Ok(mut state) = self.state.write() else {
            return false;
        };
        let before = state.entries.len();
        state
            .entries
            .retain(|e| !(e.provider.as_str() == provider && e.id.as_str() == id));
        if state.entries.len() == before {
            return false;
        }
        // `if (models.size === 0) this.virtualModels.delete(providerId)` (`:978`).
        let provider_id = if state
            .entries
            .iter()
            .any(|e| e.provider.as_str() == provider)
        {
            ProviderId::from(provider)
        } else {
            let gone = ProviderId::from(provider);
            state.virtual_only.retain(|p| p != &gone);
            gone
        };
        drop(state);
        self.bump_and_notify(&provider_id);
        true
    }

    /// The catalog entry of one registered virtual model, if it is registered.
    ///
    /// This is the lookup `getBranchSelection`'s closure wants: a session restoring a branch has to
    /// ask whether the selection names a virtual model, and a virtual model that is no longer
    /// registered must NOT hold the selection.
    #[must_use]
    pub fn get(&self, provider: &str, id: &str) -> Option<Model> {
        self.read(|s| {
            s.entries
                .iter()
                .find(|e| e.provider.as_str() == provider && e.id.as_str() == id)
                .map(|e| e.model.clone())
        })
    }

    /// One registered virtual model as a [`cyrup_core::ModelRef`].
    ///
    /// This is the shape `cyrup_session::virtual_models::branch_selection`'s `get_model` closure
    /// needs: it tests the answer through `is_virtual_api`, which reads [`cyrup_core::ModelRef`]'s
    /// `api`. A reference built with `api: None` would make every virtual selection read as
    /// PHYSICAL, so a resumed session would silently restore the model that answered last instead
    /// of the router the user selected — which is the exact bug that walk exists to prevent.
    #[must_use]
    pub fn model_ref(&self, provider: &str, id: &str) -> Option<cyrup_core::ModelRef> {
        self.get(provider, id).map(|m| cyrup_core::ModelRef {
            provider: m.provider,
            api: Some(m.api),
            model: m.id,
        })
    }

    /// Whether `(provider, id)` is registered.
    #[must_use]
    pub fn is_registered(&self, provider: &str, id: &str) -> bool {
        self.read(|s| {
            s.entries
                .iter()
                .any(|e| e.provider.as_str() == provider && e.id.as_str() == id)
        })
    }

    /// One provider's virtual catalog rows, in registration order — pi's
    /// `[...(this.virtualModels.get(providerId)?.values() ?? [])].map((entry) => entry.model)`
    /// (`model-runtime.ts:292`). Empty when nothing is registered under that id.
    #[must_use]
    pub fn models_for(&self, provider: &str) -> Vec<Model> {
        self.read(|s| {
            s.entries
                .iter()
                .filter(|e| e.provider.as_str() == provider)
                .map(|e| e.model.clone())
                .collect()
        })
    }

    /// Every provider id that has at least one virtual model, in first-registration order — pi's
    /// `...this.virtualModels.keys()` contribution to `providerIds()` (`model-runtime.ts:155`).
    #[must_use]
    pub fn provider_ids(&self) -> Vec<ProviderId> {
        self.read(|s| {
            let mut out: Vec<ProviderId> = Vec::new();
            for e in &s.entries {
                if !out.iter().any(|p| p == &e.provider) {
                    out.push(e.provider.clone());
                }
            }
            out
        })
    }

    /// Provider ids that **only** virtual models define, and which therefore count as configured
    /// without credentials — pi's provisional `{ type: "api_key", source: "virtual" }` marking
    /// (`model-runtime.ts:962-968`).
    ///
    /// An availability filter must seed its configured set with these, or a virtual-only provider's
    /// models are listed as unavailable and a session cannot restore a selection naming one.
    /// Upstream's reason for marking eagerly rather than waiting is in its own comment: *"session
    /// restore checks auth before the refresh below lands."*
    ///
    /// A virtual model listed under a provider that DOES have physical models needs nothing here:
    /// it inherits that provider's own answer, which is upstream's "availability follows the
    /// provider's auth".
    #[must_use]
    pub fn virtual_only_providers(&self) -> Vec<ProviderId> {
        self.read(|s| s.virtual_only.clone())
    }

    /// Merge the virtual rows into a **flat** model catalog, applying pi's hide rule.
    ///
    /// This is cyrup's whole port of `withVirtualModels`' catalog effect, and the ONLY one:
    /// cyrup's composed catalog is a plain `Vec<Model>` assembled from several sources and
    /// de-duplicated by `(provider, id)`, and such a merge typically keeps the FIRST occurrence —
    /// which inverts pi's rule. Upstream is explicit that
    /// the virtual row wins: *"A virtual model hides a physical chat model with the same id, which
    /// a catalog refresh can add after registration"* (`virtual-models.ts:198-200`,
    /// `withVirtualModels`'s physical filter at `:218`). So an append is not sufficient; the
    /// colliding row must be REMOVED.
    ///
    /// Ordering matches upstream's decorator (`virtual-models.ts:224`): each virtual row lands
    /// immediately after the last row of its own provider, so a provider's physical rows come
    /// first and its virtual rows follow in registration order — upstream's
    /// `["small", "large", "auto", "fast"]`. A provider with no physical rows appends at the end,
    /// giving `["auto", "second"]`.
    ///
    /// Only chat rows are hidden, because this *is* the chat catalog; an image or classifier row
    /// sharing an id lives in a different list and survives, which is the
    /// `isModelType(model, "chat")` conjunct of `:218`.
    pub fn apply_to_catalog(&self, models: &mut Vec<Model>) {
        let entries = self.read(|s| s.entries.clone());
        if entries.is_empty() {
            return;
        }
        // The hide rule, for every registered row, before any insertion — so a provider's own
        // position is computed against its physical rows only.
        for e in &entries {
            models.retain(|m| !(m.provider == e.provider && m.id == e.id));
        }
        for e in &entries {
            let at = models
                .iter()
                .rposition(|m| m.provider == e.provider)
                .map_or(models.len(), |i| i + 1);
            models.insert(at, e.model.clone());
        }
    }

    /// Ask a virtual model's router for the physical model and thinking level of ONE request — pi
    /// `resolveModel` (`model-runtime.ts:984-1024`).
    ///
    /// Upstream's own contract: *"The router must return a physical catalog model whose provider
    /// has credentials; the thinking level is clamped to that model."*
    ///
    /// Step for step, in pi's order:
    ///
    /// 1. Look the router up; refuse with [`VirtualModelError::NotRegistered`] if there is none.
    /// 2. Build `previous` from [`find_latest_response`] mapped through
    ///    [`VirtualModelCatalog::physical_model`], so an assistant message left by FAILED ROUTING —
    ///    which names the virtual model — yields `None` rather than a bogus physical previous.
    /// 3. Build `failed` from `options.failed` the same way, for the same reason. pi's comment:
    ///    *"A failed routing attempt names the virtual model; there is no physical request to
    ///    report."*
    /// 4. Call the router. Its own failure propagates unchanged as
    ///    [`VirtualModelError::Route`].
    /// 5. Re-resolve the answer: [`VirtualModelError::NotPhysical`] when it is not a physical
    ///    catalog model (unknown, or another virtual model), then
    ///    [`VirtualModelError::NoCredentials`] when its provider has none.
    /// 6. Return the CATALOG's model — not the router's — with the level clamped to it by
    ///    [`clamp_thinking_level`], and the router's `state` untouched.
    ///
    /// # Calling this from the agent loop
    ///
    /// Everything this needs that is not in the catalog comes in through [`RouteOptions`], and the
    /// caller owns all four:
    ///
    /// - `reason` — `Retry` when a failed response is stashed, else `User` when any message the
    ///   user wrote follows the last assistant message, else `Continuation`. A request made outside
    ///   the agent loop is `Direct`.
    /// - `thinking_level` — the SESSION's selected level, which is the level the user chose on the
    ///   virtual model.
    /// - `failed` — the stashed failed response, taken and cleared.
    /// - `state` — the branch's stored router state for this virtual model.
    ///
    /// Two things this does NOT do, deliberately. It does not read or write the session: the state
    /// goes in and comes back out, and persisting [`ModelRoute::state`] (only when it differs, and
    /// never for `Direct`) is the caller's. And it does not touch the SELECTION: the session still
    /// has the virtual model selected afterwards, which is what makes the route a per-request
    /// override.
    ///
    /// # Errors
    ///
    /// Any [`VirtualModelError`] above. Upstream's contract for all of them is one sentence: *"If
    /// `route()` throws, or returns a virtual model or a model without credentials, the request
    /// ends with an error response."*
    pub async fn resolve_model(
        &self,
        model: &Model,
        messages: &[Message],
        options: RouteOptions<'_>,
        catalog: &dyn VirtualModelCatalog,
    ) -> Result<ModelRoute, VirtualModelError> {
        // `const virtual = this.virtualModels.get(model.provider)?.get(model.id); if (!virtual)
        // throw ...` (`:996-998`). The guard is released before the router is called.
        let router = self.read(|s| {
            s.entries
                .iter()
                .find(|e| e.provider == model.provider && e.id == model.id)
                .map(|e| e.router.clone())
        });
        let Some(router) = router else {
            return Err(VirtualModelError::NotRegistered {
                provider: model.provider.clone(),
                id: model.id.clone(),
            });
        };
        // `const latest = findLatestResponse(messages); const previousModel = latest &&
        // this.getPhysicalModel(latest.provider, latest.model)` (`:1000-1001`).
        let latest = find_latest_response(messages);
        let previous = latest
            .and_then(|a| catalog.physical_model(a.provider.as_str(), a.model.as_str()))
            .map(|m| RoutedModel {
                model: m,
                thinking_level: latest.and_then(message_thinking_level),
            });
        // `const failedModel = failed && this.getPhysicalModel(failed.provider, failed.model)`
        // (`:1003`) — the virtual-named failure maps to undefined, hence no `failed` at all.
        let failed = options.failed.and_then(|f| {
            catalog
                .physical_model(f.provider.as_str(), f.model.as_str())
                .map(|m| FailedRequest {
                    model: m,
                    thinking_level: message_thinking_level(f),
                    message: f.clone(),
                })
        });
        let route = router
            .route(ModelRouteRequest {
                model,
                thinking_level: options.thinking_level,
                reason: options.reason,
                previous,
                failed,
                state: options.state,
                messages,
                cancel: options.cancel,
                catalog,
            })
            .await?;
        // `const target = this.getPhysicalModel(route.model.provider, route.model.id)` (`:1019`)
        // — the router's own `Model` is only an address; the CATALOG's row is what is returned.
        let target_provider = route.model.provider.clone();
        let target_id = route.model.id.clone();
        let Some(target) = catalog.physical_model(target_provider.as_str(), target_id.as_str())
        else {
            return Err(VirtualModelError::NotPhysical {
                provider: model.provider.clone(),
                id: model.id.clone(),
                target_provider,
                target_id,
            });
        };
        if !catalog.has_configured_auth(target.provider.as_str()) {
            return Err(VirtualModelError::NoCredentials {
                provider: model.provider.clone(),
                id: model.id.clone(),
                target_provider,
                target_id,
            });
        }
        // `{ model: target, thinkingLevel: clampThinkingLevel(target, route.thinkingLevel),
        // state: route.state }` (`:1024`).
        Ok(ModelRoute {
            thinking_level: clamp_thinking_level(&target, route.thinking_level),
            model: target,
            state: route.state,
        })
    }

    fn read<T>(&self, f: impl FnOnce(&RegistryState) -> T) -> T
    where
        T: Default,
    {
        self.state.read().map(|s| f(&s)).unwrap_or_default()
    }

    /// pi's mutator tail: bump the cache key, then hand the listener the provider that changed.
    fn bump_and_notify(&self, provider: &ProviderId) {
        self.generation.fetch_add(1, Ordering::Release);
        let listener = self
            .listener
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().cloned());
        if let Some(listener) = listener {
            listener.virtual_models_changed(provider);
        }
    }
}

/// The per-request inputs of [`VirtualModelRegistry::resolve_model`] — pi's `options` argument
/// (`model-runtime.ts:1006-1012`).
#[derive(Debug)]
pub struct RouteOptions<'a> {
    pub reason: ModelRouteReason,
    /// The SESSION's selected thinking level, not a routed one.
    pub thinking_level: ModelThinkingLevel,
    /// The failed response a retry is retrying. `messages` must no longer contain it, which is
    /// upstream's invariant for the field.
    pub failed: Option<&'a AssistantMessage>,
    /// The router state stored on the session branch. Always `None` for
    /// [`ModelRouteReason::Direct`].
    pub state: Option<&'a Value>,
    pub cancel: CancelToken,
}

impl RouteOptions<'_> {
    /// The minimal form: a reason, a level, and nothing stashed or stored.
    #[must_use]
    pub fn new(reason: ModelRouteReason, thinking_level: ModelThinkingLevel) -> Self {
        Self {
            reason,
            thinking_level,
            failed: None,
            state: None,
            cancel: CancelToken::new(),
        }
    }
}

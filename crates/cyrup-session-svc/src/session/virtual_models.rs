//! The virtual-model ROUTING STEP — pi's `_installAgentRequestProjection` routing arm
//! (`packages/coding-agent/src/core/agent-session.ts:775-831` @v1.0.4) plus the two reads the
//! registry takes off `this` (`getPhysicalModel`, `:1026-1030`; `hasConfiguredAuth`, `:543-545`)
//! and the `direct` arm that sizes a summarization request
//! (`_getSummarizationRequestAuth`, `:560-596`).
//!
//! # What routes where
//!
//! A **virtual model** is a catalog entry that routes each request to a physical model. The
//! SELECTION stays virtual — `AgentSession::model`, the agent's `state.model` and the branch's
//! `model_change` all keep naming it — and only the REQUEST is overridden. Providers therefore only
//! ever see physical models, and assistant messages record the physical model that answered.
//!
//! Upstream has exactly two routing sites and cyrup has the same two:
//!
//! | upstream | cyrup | reason |
//! |---|---|---|
//! | `agent.prepareRequest` override (`:775-831`) | [`AgentSession::route_request`], called from `PolicyHooks::prepare_request` | `User` / `Continuation` / `Retry` |
//! | `_getSummarizationRequestAuth` (`:560-596`) and `ModelRuntime.streamSimple`'s virtual arm (`model-runtime.ts:717-733`) | [`AgentSession::summarization_model`] | `Direct` |
//!
//! # Why the selection is read off the AGENT and not off the request
//!
//! cyrup folds a [`cyrup_agent::RequestUpdate`] into the run baseline STICKILY
//! (`cyrup-agent/src/agent/run/turn.rs`, pi `agent-loop.ts:218-238`), so after the first routed
//! request `PrepareRequestCtx::model` holds the PHYSICAL model the router picked. Reading it would
//! route once and then re-route the router's own answer — which is a request for a model whose api
//! is not `pi-virtual` and so no route at all. pi re-reads `this.agent.state.model` on every
//! request for exactly this reason (`:800`), and so does this.
//!
//! # Locks
//!
//! The router is arbitrary async code and may call back into the catalog (upstream's own example
//! router classifies through the model registry), so the session's `AsyncMutex<SessionManager>` is
//! NEVER held across `resolve_model`: the branch state is read under a guard that is dropped before
//! the await, and the state entry is appended afterwards through
//! [`AgentSession::append_custom_entry`], which takes the lock itself. The registry's own `RwLock`
//! is likewise never held across the await ([`cyrup_provider::VirtualModelRegistry`] clones the
//! router `Arc` out first).

use std::sync::Arc;

use cyrup_agent::AgentMessage;
use cyrup_core::{AssistantMessage, CancelToken, ModelRef, ModelThinkingLevel};
use cyrup_provider::{
    Model, ModelRoute, ModelRouteReason, RouteOptions, VirtualModelCatalog, VirtualModelDefinition,
    VirtualModelError, is_virtual_model,
};
use cyrup_session::virtual_models::{
    VIRTUAL_MODEL_STATE_ENTRY, is_virtual_api, virtual_model_state,
};

use crate::error::SessionServiceError;

use super::AgentSession;

/// A frozen view of the session's catalog, handed to the registry as its
/// [`VirtualModelCatalog`] — pi's two reads off `this` (`model-runtime.ts:957`, `:1019-1021`).
///
/// It is a SNAPSHOT and not a live accessor for one concrete reason:
/// [`cyrup_provider::VirtualModelRegistry::register`] holds its own write lock while it asks
/// `has_physical_provider` / `has_configured_auth` (pi's `!recomposeProvider(id) &&
/// !configuredProviders.has(id)`, `model-runtime.ts:962-968`). A catalog that answered those by
/// recomposing — which reads the same `RwLock` — would deadlock on the spot. Building the view
/// before the call makes the re-entrancy unrepresentable rather than merely avoided.
///
/// `models` is the composed registry WITH the virtual rows already applied (`compose_model_registry`
/// ends in `apply_to_catalog`), which is what makes [`Self::get_model`] pi's `this.models.getModel`
/// — a lookup that sees virtual entries, so re-registering an id is a replacement and not a
/// conflict.
pub(crate) struct SessionVirtualCatalog<'a> {
    session: &'a AgentSession,
    models: Arc<Vec<Model>>,
}

impl VirtualModelCatalog for SessionVirtualCatalog<'_> {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.models
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
            .cloned()
    }

    /// pi `ctx.modelRegistry.getModels()` — the SAME composed registry `get_model` looks up in,
    /// virtual rows and all, which is what a router reached through
    /// [`cyrup_provider::ModelRouteRequest::catalog`] reads.
    fn models(&self) -> Vec<Model> {
        self.models.as_ref().clone()
    }

    fn has_configured_auth(&self, provider: &str) -> bool {
        // The CREDENTIAL question alone — never the union that includes the virtual-only marking.
        // See `AgentSession::physical_provider_is_configured` for both reasons (correctness at the
        // registration site, and the re-entrancy this whole type exists to rule out).
        self.session
            .physical_provider_is_configured(&cyrup_core::ProviderId::from(provider))
            // …plus cyrup's standing offline accommodation, which every other credential check in
            // this crate makes: a provider the session is ALREADY streaming against is usable even
            // without a stored key. See `AgentSession::installed_provider_exposes`.
            || self.session.installed_provider_exposes(provider)
    }

    fn has_physical_provider(&self, provider: &str) -> bool {
        // pi's test is whether `composeProvider(id)` yields a provider at all
        // (`recomposeProvider`, `model-runtime.ts:293-297`). The composed catalog's own answer is
        // that test: a provider nothing but this registry defines contributes only virtual rows.
        self.models
            .iter()
            .any(|m| m.provider.as_str() == provider && !is_virtual_model(m))
    }
}

/// What [`AgentSession::route_request`] decided, for `PolicyHooks::prepare_request` to fold into
/// the loop's [`cyrup_agent::RequestUpdate`].
pub(crate) struct RoutedRequest {
    /// The physical model this ONE request goes to (pi `route.model`, `:830`).
    pub(crate) model: ModelRef,
    /// The level the router chose, clamped to what the routed model offers (pi `route.thinkingLevel`).
    pub(crate) thinking_level: ModelThinkingLevel,
    /// A replacement transcript, `Some` only when the routed-model threshold check compacted (pi
    /// re-runs `prepare()`, `:828`).
    pub(crate) context: Option<Vec<AgentMessage>>,
}

impl AgentSession {
    /// The session's virtual-model registry — pi `ModelRuntime.virtualModels`
    /// (`model-runtime.ts:179`). Shared; registering through this `Arc` from anywhere reaches the
    /// session's next catalog read, because the registry's `generation` is a key of the
    /// registry-snapshot cache.
    #[must_use]
    pub fn virtual_model_registry(&self) -> &Arc<cyrup_provider::VirtualModelRegistry> {
        &self.services.virtual_models
    }

    /// Register a virtual model on this session — pi `ModelRuntime.registerVirtualModel`
    /// (`model-runtime.ts:947-974`), with pi's three validations and pi's exact refusal text.
    ///
    /// The catalog view is built BEFORE the call, which is what keeps the conflict check
    /// ([`SessionVirtualCatalog`]) free of re-entrancy on the registry's own lock.
    ///
    /// # Errors
    ///
    /// [`VirtualModelError::EmptyProviderOrId`] or [`VirtualModelError::PhysicalConflict`]; nothing
    /// is registered in either case.
    pub fn register_virtual_model(
        &self,
        definition: VirtualModelDefinition,
    ) -> Result<(), VirtualModelError> {
        let catalog = self.virtual_catalog();
        self.services.virtual_models.register(definition, &catalog)
    }

    /// Remove one virtual model — pi `unregisterVirtualModel` (`model-runtime.ts:976-982`).
    /// `false` when that `(provider, id)` was not registered, which is pi's no-op.
    pub fn unregister_virtual_model(&self, provider: &str, id: &str) -> bool {
        self.services.virtual_models.unregister(provider, id)
    }

    /// The frozen catalog view the registry reads through.
    pub(crate) fn virtual_catalog(&self) -> SessionVirtualCatalog<'_> {
        SessionVirtualCatalog {
            session: self,
            models: self.full_model_registry(),
        }
    }

    /// A catalog chat model that is NOT virtual — pi `getPhysicalModel`
    /// (`model-runtime.ts:1026-1030`).
    ///
    /// This is the predicate that makes a FAILED ROUTING invisible to the router: such a message
    /// names the virtual model (`virtual-models.ts:104`), so mapping it through here yields `None`
    /// and the router is told there is no previous (or no failed) physical request — upstream's own
    /// words at `:1010-1011`.
    #[must_use]
    pub(crate) fn physical_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.full_model_registry()
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
            .filter(|m| !is_virtual_model(m))
            .cloned()
    }

    /// Whether the SELECTION names a virtual model — pi `isVirtualModel(this.model)`.
    #[must_use]
    pub(crate) fn selection_is_virtual(&self) -> bool {
        let selection = Self::lock(&self.model).clone();
        is_virtual_api(selection.as_ref().and_then(|m| m.api.as_ref()))
    }

    /// The model whose limits apply to `message`, or `None` when the message came from another
    /// model — pi `_modelForMessage` (`agent-session.ts:598-607`).
    ///
    /// Under a virtual selection that is [`Self::physical_model`] of the message's own
    /// provider/model, so the limits follow the model that ANSWERED rather than the virtual model's
    /// declared ones (which default to 0 and would read as "unknown"). Otherwise it is the
    /// selection iff the message names it.
    ///
    /// Two callers depend on this and get the WRONG answer without it, in opposite directions:
    /// `check_compaction`'s `same_model` is never true for a real response under a virtual
    /// selection (so the overflow arm and the compact-and-retry path are unreachable), and it is
    /// always true for a routing-failure message (so a failure is treated as a same-model overflow
    /// against the virtual model's window).
    #[must_use]
    pub(crate) fn model_for_message(&self, message: &AssistantMessage) -> Option<Model> {
        if self.selection_is_virtual() {
            return self.physical_model(message.provider.as_str(), message.model.as_str());
        }
        let selection = Self::lock(&self.compaction_model).clone();
        selection
            .filter(|m| m.provider == message.provider && m.id.as_str() == message.model.as_str())
    }

    /// Under a virtual selection, the physical model and thinking level of the latest successful
    /// response — pi's `get routedModel()` (`agent-session.ts:1437-1443`).
    ///
    /// `None` whenever the selection is not virtual (upstream's first clause), and also when no
    /// settled response exists yet or when the one that does names a model the catalog no longer
    /// holds — pi's `model && { model, thinkingLevel }`, where `model` comes from
    /// `getPhysicalModel`, which answers nothing for an absent row OR for a virtual one. That last
    /// case is what keeps a FAILED ROUTING from reporting the virtual model's own limits: such a
    /// message names the virtual model (`virtual-models.ts:104`), and
    /// [`Self::physical_model`] rejects it.
    ///
    /// This is the accessor the limits rule and the footer's routed suffix are both built on. The
    /// `thinking_level` is the level the loop REQUESTED for that response
    /// ([`AssistantMessage::thinking_level`]), which is `None` for a response written before that
    /// field existed — exactly upstream's own optional.
    ///
    /// Basis: `branch_path(None)` rather than upstream's `agent.state.messages`. See
    /// [`cyrup_session::virtual_models::latest_branch_response`] for why the two agree.
    #[must_use]
    pub async fn routed_model(&self) -> Option<cyrup_provider::RoutedModel> {
        if !self.selection_is_virtual() {
            return None;
        }
        // Everything the branch is asked for is extracted under the guard and the guard is then
        // dropped, because `physical_model` reads the composed catalog, which may recompose — and
        // the rule this crate follows is that no leaf read spans the manager guard.
        let (provider, model_id, thinking_level) = {
            let guard = self.manager.lock().await;
            let branch = guard.branch_path(None);
            let latest = cyrup_session::virtual_models::latest_branch_response(&branch)?;
            (
                latest.provider.to_string(),
                latest.model.to_string(),
                latest.thinking_level,
            )
        };
        let model = self.physical_model(&provider, &model_id)?;
        Some(cyrup_provider::RoutedModel {
            model,
            thinking_level,
        })
    }

    /// The model whose limits apply to the conversation — pi `_limitsModel()`
    /// (`agent-session.ts:627-630`): `this.routedModel?.model ?? this.model`.
    ///
    /// This is the whole of `docs/virtual-models.md:29`'s first two sentences. Under a virtual
    /// selection the limits follow the PHYSICAL model that produced the latest response, even when
    /// that response pre-dates the switch to the virtual model; with no such response the `??` arm
    /// hands back the selection, i.e. the limits DECLARED on the virtual model — which
    /// [`cyrup_provider::create_virtual_model`] defaults to 0, and a 0 window is "unknown" at every
    /// reader.
    ///
    /// For a physical selection [`Self::routed_model`] is `None` by its first clause, so this is
    /// the selection and every existing number is unchanged.
    pub(crate) async fn limits_model(&self) -> Option<Model> {
        if let Some(routed) = self.routed_model().await {
            return Some(routed.model);
        }
        Self::lock(&self.compaction_model).clone()
    }

    /// Take and clear the stashed failed response — pi's first two statements in the
    /// `prepareRequest` override (`agent-session.ts:778-779`).
    pub(crate) fn take_failed_response(&self) -> Option<AssistantMessage> {
        Self::lock(&self.failed_response).take()
    }

    /// Stash the response a retry is about to retry — pi's `this._failedResponse = message`
    /// (`agent-session.ts:1852` after `_prepareRetry` succeeds, `:3031` after an overflow
    /// compact-and-retry).
    ///
    /// The retry paths have already dropped it from the agent transcript
    /// (`drop_trailing_assistant`), which is upstream's invariant for the field: `messages` no
    /// longer contains the failed request, so the router is handed it separately.
    pub(crate) fn stash_failed_response(&self, message: &AssistantMessage) {
        *Self::lock(&self.failed_response) = Some(message.clone());
    }

    /// Clear the stash — pi's `this._failedResponse = undefined` at the head of `_runAgentPrompt`
    /// (`:1811`, "Compaction before the prompt may have scheduled a retry; the new prompt replaces
    /// it") and in its `finally` (`:1831`).
    pub(crate) fn clear_failed_response(&self) {
        *Self::lock(&self.failed_response) = None;
    }

    /// Record the SELECTION on the current branch when the branch implies another one, so a
    /// resume restores it — pi `_recordSelection` (`agent-session.ts:609-625`), new at v1.0.x
    /// (there is no `_recordSelection` in `agent-session.ts` at v0.87.1).
    ///
    /// Upstream's own words: *"Tree navigation can leave the latest `model_change` on another
    /// branch; responses cannot record a virtual selection because they name physical models.
    /// Responses do record a physical selection unless the branch holds a virtual one; checking a
    /// physical selection against responses would record it on every prompt while `prepareRequest`
    /// redirects to another model."*
    ///
    /// The guard is the whole point of the function. Without the last clause
    /// (`!isVirtualModel(model) && !(recordedModel && isVirtualModel(recordedModel))`, `:623`) a
    /// `model_change` would be appended on EVERY prompt of a routed session, because the branch's
    /// recorded selection is the virtual model while the request was redirected to a physical one.
    ///
    /// Infallible upstream, so a write failure is SWALLOWED rather than failing the prompt: pi's
    /// `appendModelChange` is synchronous and has no failure path at this call site, and a prompt
    /// that refuses to start because a bookkeeping entry could not be written would be a worse
    /// behaviour than a resume that restores one selection too old.
    pub(super) async fn record_selection(&self) {
        let Some(model) = Self::lock(&self.model).clone() else {
            return;
        };
        // Virtual-only, as everywhere else in this port: `branch_selection` consumes the answer
        // only through `is_virtual_api`, and so does the `recorded` test below — see
        // `cyrup_session::virtual_models::branch_selection`'s proof.
        let registry = Arc::clone(&self.services.virtual_models);
        let lookup = |p: &str, id: &str| registry.model_ref(p, id);
        let mut manager = self.manager.lock().await;
        let Some(recorded) = manager.branch_selection(lookup) else {
            return;
        };
        if recorded.provider == model.provider && recorded.model == model.model {
            return;
        }
        let recorded_is_virtual = lookup(recorded.provider.as_str(), recorded.model.as_str())
            .is_some_and(|m| is_virtual_api(m.api.as_ref()));
        if !is_virtual_api(model.api.as_ref()) && !recorded_is_virtual {
            return;
        }
        if let Err(e) = manager.append_model_change(model.provider.clone(), model.model.clone()) {
            tracing::warn!(error = %e, "could not record the session model selection");
        }
    }

    /// The routing step — pi `_installAgentRequestProjection`'s virtual arm
    /// (`agent-session.ts:798-830`), minus the canonical-projection rebuild, which is AGENT-038 and
    /// deliberately still unported.
    ///
    /// `Ok(None)` means the selection is not virtual and the caller must pass its own update
    /// through unchanged — pi's `if (!isVirtualModel(model)) return {...previous, ...}` (`:802`).
    ///
    /// Order is upstream's, and each step is where it is for a reason:
    ///
    /// 1. derive the reason from the transcript (`:807-811`);
    /// 2. read the branch's stored router state (`:809`) — under a guard that is dropped before (3);
    /// 3. route (`:812-816`) — nothing is written yet, so a router that fails writes nothing;
    /// 4. append the new state (`:817-823`) — BEFORE the request, so it survives a later failure;
    /// 5. compact if the ROUTED model's window is too small (`:824-829`) — the route stands;
    /// 6. install the routed model's owning provider (cyrup-only; see below).
    ///
    /// Step 6 has no upstream counterpart because pi keeps every provider live and dispatches on
    /// `model.provider` inside its own `streamFn`. cyrup installs exactly ONE provider at a time
    /// and `ProviderStreamFn` resolves the request's model id against THAT provider's catalog, so a
    /// cross-provider route must swap it or the request streams against the wrong provider.
    ///
    /// # Errors
    ///
    /// [`SessionServiceError::VirtualModelRouting`] for every refusal the registry can produce — an
    /// unregistered virtual model, a router that failed, a route to another virtual model or to one
    /// with no credentials. The caller turns it into a hook error, which ends the run with an error
    /// response that still names the VIRTUAL model (see
    /// [`cyrup_core::VIRTUAL_MODEL_API`]), and that is exactly what
    /// `cyrup_session::virtual_models::branch_selection` skips when it restores the selection.
    pub(crate) async fn route_request(
        &self,
        messages: &[Arc<AgentMessage>],
        override_model: Option<ModelRef>,
        override_level: Option<ModelThinkingLevel>,
        failed: Option<AssistantMessage>,
        cancel: CancelToken,
    ) -> Result<Option<RoutedRequest>, SessionServiceError> {
        // pi reads the model and the level off `agent.state`, with an override the inner
        // `prepareRequest` returned taking precedence (`previous?.model ?? this.agent.state.model`,
        // `:800-801`). NEVER off the request — see the module docs.
        let snapshot = self.agent.snapshot().await;
        let Some(selection) = override_model.or(snapshot.model) else {
            return Ok(None);
        };
        if !is_virtual_api(selection.api.as_ref()) {
            return Ok(None);
        }
        let thinking_level = override_level.unwrap_or(snapshot.thinking_level);
        let provider = selection.provider.as_str().to_string();
        let model_id = selection.model.as_str().to_string();
        // The virtual catalog row. pi hands `resolveModel` the `Model` the agent holds; cyrup's
        // agent holds a `ModelRef`, so the row comes from the registry — and its absence is
        // upstream's `Virtual model p/i is not registered.` either way.
        let Some(virtual_model) = self.services.virtual_models.get(&provider, &model_id) else {
            return Err(SessionServiceError::VirtualModelRouting(
                VirtualModelError::NotRegistered {
                    provider: selection.provider.clone(),
                    id: selection.model.clone(),
                }
                .to_string(),
            ));
        };

        // `const lastResponse = context.messages.findLastIndex(m => m.role === "assistant");`
        // `const userTurn = context.messages.slice(lastResponse + 1).some(m => m.role === "user");`
        // (`:807-808`), with upstream's own comment: "Only messages the user wrote start a turn;
        // extension messages can follow them, e.g. from before_agent_start."
        //
        // `.any()` over the WHOLE tail and not "the last message is a user message" is what keeps a
        // hidden message an extension appended after the prompt from downgrading `User` to
        // `Continuation`; and `AgentMessage::User` and nothing else is what keeps an extension's
        // injected `Custom`/`App` message from promoting a continuation to `User`.
        let last_response = messages
            .iter()
            .rposition(|m| matches!(m.as_ref(), AgentMessage::Assistant(_)));
        let tail = messages
            .get(last_response.map_or(0, |i| i + 1)..)
            .unwrap_or(&[]);
        let user_turn = tail
            .iter()
            .any(|m| matches!(m.as_ref(), AgentMessage::User { .. }));
        let reason = if failed.is_some() {
            ModelRouteReason::Retry
        } else if user_turn {
            ModelRouteReason::User
        } else {
            ModelRouteReason::Continuation
        };

        // `getVirtualModelState(this.sessionManager.getBranch(), model.provider, model.id)`
        // (`:809`). The guard is scoped so it is released before the router runs.
        let state = {
            let manager = self.manager.lock().await;
            virtual_model_state(&manager.branch_path(None), &provider, &model_id).cloned()
        };

        // `resolveModel(model, convertToLlm(context.messages), {...})` (`:812-816`) — the ROUTER
        // sees the request's own transcript, converted exactly as the request itself would be.
        let llm = crate::hooks::coding_agent_convert_to_llm(messages);
        let catalog = self.virtual_catalog();
        let route = self
            .services
            .virtual_models
            .resolve_model(
                &virtual_model,
                &llm,
                RouteOptions {
                    reason,
                    thinking_level,
                    failed: failed.as_ref(),
                    state: state.as_ref(),
                    cancel,
                },
                &catalog,
            )
            .await
            .map_err(|e| SessionServiceError::VirtualModelRouting(e.to_string()))?;
        drop(catalog);

        self.persist_router_state(&provider, &model_id, state.as_ref(), route.state.as_ref())
            .await?;

        // "The route stands: the router already decided this request." (`:824-826`.) The threshold
        // is checked against the ROUTED model's window, and a compaction replaces the transcript
        // without re-asking the router.
        let context = self.compact_before_routed_request(&route.model).await?;

        self.install_owning_provider(&route.model)?;
        Ok(Some(RoutedRequest {
            model: route_model_ref(&route),
            thinking_level: route.thinking_level,
            context,
        }))
    }

    /// Store the router's new state on the branch — pi `:817-823`.
    ///
    /// Written only when the router returned something AND it differs from what was handed in,
    /// which is pi's `route.state !== undefined && route.state !== state`. It lands BEFORE the
    /// request is sent, so the state a request was routed with survives that request failing or
    /// being aborted — upstream states this outright ("The state stays stored if the request later
    /// fails").
    ///
    /// `[CYRUP-DELTA]` pi's second test is REFERENCE inequality: a router that returns a freshly
    /// built object equal to the current state stores a duplicate entry upstream and stores nothing
    /// here. Rust has no stable identity for a returned [`serde_json::Value`], so the comparison is
    /// by VALUE. Upstream documents the behaviour it wants in terms of the caller — "Return a new
    /// object only when the state changes" — and its own test asserts only the two DISTINCT states,
    /// so nothing observable depends on the duplicate.
    async fn persist_router_state(
        &self,
        provider: &str,
        model_id: &str,
        previous: Option<&serde_json::Value>,
        next: Option<&serde_json::Value>,
    ) -> Result<(), SessionServiceError> {
        let Some(next) = next else { return Ok(()) };
        if previous == Some(next) {
            return Ok(());
        }
        // pi `VirtualModelStateData { provider, modelId, state }` (`virtual-models.ts:36-40`), in
        // pi's camelCase — which is exactly what `cyrup_session::virtual_models::virtual_model_state`
        // matches on, so a pi-written entry and a cyrup-written one are the same bytes.
        let data = serde_json::json!({
            "provider": provider,
            "modelId": model_id,
            "state": next,
        });
        self.append_custom_entry(VIRTUAL_MODEL_STATE_ENTRY, data)
            .await?;
        Ok(())
    }

    /// Route a request made OUTSIDE the agent loop — pi's `direct` arm
    /// (`_getSummarizationRequestAuth`, `agent-session.ts:560-581`, whose own comment is *"Route a
    /// virtual model first: summaries size their input and output from the model they get"*, and
    /// `ModelRuntime.streamSimple`'s virtual arm, `model-runtime.ts:717-733`).
    ///
    /// Answers the `(model, thinking_level)` a summarization request must be built with: the
    /// selection and the session level when it is physical, and the ROUTED pair when it is virtual.
    /// `None` is a modelless session, which every caller already handles.
    ///
    /// Three things are deliberately different from [`Self::route_request`], all of them upstream's:
    /// the reason is [`ModelRouteReason::Direct`]; no state is read; and any state the router
    /// returns is DISCARDED ("`direct` requests have no state, and Pi ignores state they return").
    /// It is also called once per compaction rather than once per summarization call, which is what
    /// upstream's own suite pins — one `direct` route for two summary calls.
    ///
    /// # Errors
    ///
    /// [`SessionServiceError::VirtualModelRouting`], or a provider-install failure.
    pub(crate) async fn summarization_model(
        &self,
    ) -> Result<Option<(Model, ModelThinkingLevel)>, SessionServiceError> {
        let Some(selected) = ({ Self::lock(&self.compaction_model).clone() }) else {
            return Ok(None);
        };
        let level = self.thinking_level().await;
        if !is_virtual_model(&selected) {
            return Ok(Some((selected, level)));
        }
        let messages = crate::hooks::coding_agent_convert_to_llm(
            &self
                .raw_context_messages()
                .await
                .into_iter()
                .map(|m| Arc::new(crate::event::raw_message_to_agent(&m)))
                .collect::<Vec<_>>(),
        );
        let catalog = self.virtual_catalog();
        let route = self
            .services
            .virtual_models
            .resolve_model(
                &selected,
                &messages,
                RouteOptions::new(ModelRouteReason::Direct, level),
                &catalog,
            )
            .await
            .map_err(|e| SessionServiceError::VirtualModelRouting(e.to_string()))?;
        drop(catalog);
        self.install_owning_provider(&route.model)?;
        Ok(Some((route.model, route.thinking_level)))
    }
}

/// The routed model as a [`ModelRef`] for the loop's request override. `api` is populated from the
/// catalog row, so nothing downstream mistakes it for an unresolved selection.
fn route_model_ref(route: &ModelRoute) -> ModelRef {
    ModelRef {
        provider: route.model.provider.clone(),
        api: Some(route.model.api.clone()),
        model: route.model.id.clone(),
    }
}

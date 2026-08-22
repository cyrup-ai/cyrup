//! Model selection, the model catalogs and model cycling.
//!
//! Pi `agent-session.ts` `setModel`/`scopedModels`/`cycleModel` + sdk.ts's provider install.
//! Resolving a `/model` pattern, installing the owning provider into the live
//! [`crate::ProviderSwap`], the configured-auth checks that gate a candidate, attribution headers,
//! and the `cycle_model` rotation over the scoped or available set.

use cyrup_core::{ModelId, ModelRef, ModelThinkingLevel, ProviderId};
use cyrup_ext::HostEvent;
use cyrup_provider::Model;

use crate::error::SessionServiceError;
use crate::event::AgentSessionEvent;

use super::AgentSession;
use super::types::{ModelCycleResult, ScopedModel};

impl AgentSession {
    /// Switch the active model by pattern (`provider/id[:level]`), updating the agent, the
    /// compaction model, and recording a model-change entry (R-11-014 `set_model`).
    ///
    /// The pattern resolves against the FULL multi-provider registry (Pi resolves against the whole
    /// `modelRegistry`, not just the active provider) so a `/model` selection targeting a DIFFERENT
    /// provider than the current one resolves cleanly; [`Self::set_model_resolved`] then swaps the
    /// owning provider. Falls back to the current provider's own catalog for custom-id / offline faux
    /// models that are not part of the built-in registry.
    pub async fn set_model(&self, pattern: &str) -> Result<ModelRef, SessionServiceError> {
        let resolved = {
            let candidates = self.full_model_registry();
            let resolver = cyrup_config::ModelResolver::new(&candidates);
            resolver.parse_pattern(pattern, true).model
        }
        .ok_or_else(|| SessionServiceError::ModelNotFound(pattern.to_string()))?;
        self.set_model_resolved(resolved).await
    }

    /// Switch to a resolved [`Model`] (Pi `setModel(Model)`, agent-session.ts:1448-1463), running the
    /// `hasConfiguredAuth` precheck first. When the target model's provider differs from the currently
    /// installed one, the owning provider is resolved (env-backed credentials installed) and swapped
    /// into the agent's stream source in place — 1:1 with Pi switching model+provider together.
    /// Updates the agent + compaction model + attribution headers + host-services view and records a
    /// `model_change` entry.
    pub async fn set_model_resolved(&self, model: Model) -> Result<ModelRef, SessionServiceError> {
        if !self.has_configured_auth(&model) {
            return Err(SessionServiceError::NoConfiguredAuth(format!(
                "{}/{}",
                model.provider.as_str(),
                model.id.as_str()
            )));
        }
        self.install_owning_provider(&model)?;
        let previous = Self::lock(&self.model).clone();
        self.apply_model_change(&model, previous.as_ref(), "set", None).await?;
        Ok(ModelRef {
            provider: model.provider.clone(),
            api: Some(model.api.clone()),
            model: model.id.clone(),
        })
    }

    /// Cross-provider select: rebuild + install the owning provider so the agent loop streams
    /// against it (Pi switches model+provider live). A same-provider change is a no-op.
    ///
    /// Pi does not need this step: its `ModelRuntime` keeps every provider live and dispatches on
    /// `model.provider` inside `prepareRequest` (model-runtime.ts:445-470), so `setModel` and
    /// `cycleModel` alike are a bare `agent.state.model = next` assignment. cyrup installs exactly
    /// ONE provider at a time, so every path that can land on a model owned by another provider —
    /// [`Self::set_model_resolved`] **and** both `cycle_model` arms, whose candidate sets span the
    /// whole auth-filtered registry — has to swap it here or the next turn streams against the
    /// wrong provider.
    fn install_owning_provider(&self, model: &Model) -> Result<(), SessionServiceError> {
        if self.provider.current().id().as_str() == model.provider.as_str() {
            return Ok(());
        }
        // A guest-registered provider is already a realized `Provider` in the shared registry
        // (arch-08 §5.6); install it DIRECTLY so its models stream — the built-in
        // `ProviderResolver` seam (bin `select_provider`) knows only the Pi registry, not a guest
        // provider. Falls back to the resolver for a built-in cross-provider swap.
        if let Some(guest) = self.services.guest_providers.provider(model.provider.as_str()) {
            self.provider.store(guest);
        } else {
            self.provider.resolve_and_store(model.provider.as_str()).map_err(|e| {
                SessionServiceError::NoConfiguredAuth(format!(
                    "{}/{}: {e}",
                    model.provider.as_str(),
                    model.id.as_str()
                ))
            })?;
        }
        Ok(())
    }

    /// Whether the model has usable auth (Pi `modelRegistry.hasConfiguredAuth`, agent-session.ts:1449
    /// / model-registry.ts:658-664). Pi's check: the model's provider has configured auth — a stored
    /// credential, a runtime `--api-key`, or a known env var (e.g. `TOGETHER_API_KEY` for `together`).
    /// cyrup layers its offline-faux accommodation on top: a model the CURRENT injected provider
    /// exposes in its catalog is always usable (the scripted faux provider needs no key), so the
    /// active/offline model stays selectable exactly as before.
    pub fn has_configured_auth(&self, model: &Model) -> bool {
        if self.provider_has_configured_auth(&model.provider) {
            return true;
        }
        // A guest-registered provider carries its own credentials (apiKey/oauth in the registration,
        // Pi `providerRequestConfigs`, model-registry.ts:659-662), so its models are always available
        // in the selector — exactly as Pi's `hasConfiguredAuth` returns true when a provider request
        // config supplies a key.
        if self.services.guest_providers.has_provider(model.provider.as_str()) {
            return true;
        }
        self.provider
            .current()
            .models()
            .iter()
            .any(|m| m.provider == model.provider && m.id == model.id)
    }

    /// Whether `provider` has configured auth in the Pi sense — a stored credential / runtime
    /// `--api-key` / known env var (`env_keys`, e.g. `together` → `TOGETHER_API_KEY`), **or** a
    /// `models.json` block of its own carrying a configured `apiKey`.
    ///
    /// Both tiers live in one place, [`cyrup_config::provider_is_configured`], shared with the
    /// binary's default-launch predicate (`main.rs`) — the two used to be written out separately and
    /// had drifted, which is CFG-022. The models.json tier stays PRESENCE-ONLY: it never resolves the
    /// value, so a `!command` `apiKey` cannot execute a shell command on a status query; see that
    /// function's docs for why Pi's own check (provider-composer.ts:320-329) is pure too.
    ///
    /// Does NOT count the offline faux accommodation or guest-registered providers —
    /// [`Self::has_configured_auth`] adds those separately.
    fn provider_has_configured_auth(&self, provider: &ProviderId) -> bool {
        cyrup_config::provider_is_configured(
            &self.services.auth,
            &self.services.model_config,
            provider,
            None,
        )
    }

    /// pi's `checkAuth` SECOND CHANCE — `(await this._modelRuntime.checkAuth(provider)) !== undefined`
    /// (`agent-session.ts:1184` @v0.83.0). PROV-037.
    ///
    /// [`Self::has_configured_auth`] is the cached, synchronous `hasConfiguredAuth` half
    /// (`model-runtime.ts:372-374`, a `Set` lookup on a snapshot). This is the live half upstream
    /// ORs with it, and the OR is the whole point: a credential that exists but is not in the
    /// cached set — an `auth.json` written by `cyrup auth` in another terminal after this session
    /// started, an `/login` in a sibling process — lets the turn PROCEED where cyrup refused it.
    ///
    /// `[CYRUP-DELTA]` **mechanism.** pi re-runs `Models.checkAuth`, which resolves the provider's
    /// auth strategy from scratch. Reaching `cyrup_provider::Models::check_auth` from here would
    /// mean composing a whole registry per refusal (the session holds one installed `Provider`, not
    /// a `Models`), and the only reason upstream's re-check can differ from its cached answer is
    /// that the SNAPSHOT is stale — so this refreshes the snapshot ([`AuthStore::reload`], pi's own
    /// `AuthStorage.reload`, `auth-storage.ts:204-215`) and re-asks the same predicate. That covers
    /// every credential source `has_configured_auth` knows and reproduces the observable
    /// difference; what it does not cover is a provider whose auth is ambient in a way
    /// `provider_is_configured` cannot see (Bedrock's IAM chain, Vertex ADC), which is a gap in the
    /// PREDICATE and identical before and after this call.
    ///
    /// Runs only on the refusal path, so the file read costs nothing on a normal turn.
    pub(super) async fn recheck_provider_auth(&self, model: &Model) -> bool {
        self.services.auth.reload();
        self.has_configured_auth(model)
    }

    /// pi `ModelRuntime.isUsingOAuth(providerId)` — `this.snapshot.auth.get(providerId)?.type ===
    /// "oauth"` (`model-runtime.ts:368-370` @v0.83.0). PROV-037.
    ///
    /// Reads the STORED credential's type, which is exactly what upstream's snapshot holds. A read
    /// failure or an absent credential is `false`, so the refusal falls to the api-key message —
    /// the same direction upstream's `?.` takes.
    pub(super) async fn provider_is_oauth_backed(&self, provider: &ProviderId) -> bool {
        matches!(
            self.services.auth.read(provider).await,
            Ok(Some(cyrup_config::Credential::Oauth { .. }))
        )
    }

    /// Public view of [`Self::full_model_registry`] — every model the session can resolve, before
    /// the configured-auth filter [`Self::available_model_catalog`] applies.
    pub fn full_model_catalog(&self) -> Vec<Model> {
        self.full_model_registry()
    }

    /// The FULL multi-provider model registry, deduped by `provider/id`: the session's own installed
    /// provider + guest-registered providers + the compiled-in built-in catalogs, with
    /// `<agent_dir>/models.json` composed over the whole union LAST — Pi's single composed registry
    /// (`ModelRuntime.rebuildProviders`, model-runtime.ts:225-231). This is the resolution /
    /// enumeration source that spans providers, independent of which single provider is installed.
    pub(super) fn full_model_registry(&self) -> Vec<Model> {
        // --- BASE layer, in Pi's `recomposeProvider` precedence (model-runtime.ts:201) ---
        // `base = nativeExtensionProviders.get(id) ?? builtins.get(id)`: a registered provider
        // shadows the compiled-in catalog, and the compiled-in catalog fills in the rest. The
        // session's own installed provider comes first because it also carries the offline faux
        // models and any custom-id model that is not a registry entry.
        let mut base: Vec<Model> = self.provider.current().models().to_vec();
        // Guest-registered providers (Pi folds `registerProvider` models into the same `ModelRegistry`
        // that `find`/`getAvailable`/`setModel` read, model-registry.ts:917-940).
        for m in self.services.guest_providers.models() {
            if !base.iter().any(|e| e.provider == m.provider && e.id == m.id) {
                base.push(m);
            }
        }
        for m in cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
            credentials: None,
            auth_context: None,
            // The pi.dev overlay loaded once at session-build time (DRIFT-007). Already in memory,
            // so this SYNC, hot registry read stays free of disk and network I/O.
            catalog_overlay: self.services.catalog_overlay.clone(),
        })
        .get_models(None)
        {
            if !base.iter().any(|e| e.provider == m.provider && e.id == m.id) {
                base.push(m);
            }
        }
        // --- TOP layer: `<agent_dir>/models.json` (CFG-002) ---
        // Pi composes LAST and REPLACES the provider in the collection
        // (`this.models.setProvider(composeModelProvider(...))`, model-runtime.ts:215), so the
        // overlay reaches EVERY consumer — including the provider the session is currently running
        // on, which is the whole point of a `baseUrl` / `compat` / `modelOverrides` block ("point my
        // provider at a proxy", "raise contextWindow on the model I'm using"). Composing over the
        // union rather than over the compiled-in catalogs alone is what keeps the current provider's
        // uncomposed entries from shadowing their composed counterparts. Composition errors were
        // already reported at startup (`StartupDiagnostics::models`); here a rejected provider block
        // simply keeps its built-ins.
        let (composed, _errors) = self.services.model_config.compose(&base);
        composed
    }

    /// The models the `/model` selector offers: the FULL registry filtered to CONFIGURED providers
    /// (Pi `modelRegistry.getAvailable()` = `getAll().filter(hasConfiguredAuth)`,
    /// model-registry.ts:644-646, surfaced by the selector at model-selector.ts:152). A provider is
    /// configured when it has a stored credential / runtime `--api-key` / known env var (so `together`
    /// appears once `TOGETHER_API_KEY` is set), plus cyrup's offline-faux accommodation keeps the
    /// current provider's own catalog (the scripted faux default) selectable. Deduped by `provider/id`.
    pub fn available_model_catalog(&self) -> Vec<Model> {
        self.full_model_registry().into_iter().filter(|m| self.has_configured_auth(m)).collect()
    }

    /// The provider-attribution + session-affinity headers this session attaches to provider requests
    /// for `model` (Pi `mergeProviderAttributionHeaders`, sdk.ts:323; #20). Computed from the merge
    /// function + the session's telemetry flag + id. The builder threads the resolved model's headers
    /// onto the agent at construction; this getter lets callers inspect/recompute them per model.
    pub fn attribution_headers(&self, model: &Model) -> Option<cyrup_provider::HeaderMap> {
        crate::attribution::merge_provider_attribution_headers(
            model,
            self.telemetry_enabled,
            Some(&self.session_id),
            &[],
        )
    }

    /// The attribution overlay for the model a TURN is actually going to, addressed by the loop's
    /// [`ModelRef`] rather than by a resolved [`Model`] (AGENT-029). This is the body of the
    /// `transform_headers` resolver installed in [`Self::into_shared`] — pi's per-request
    /// `transformHeaders` closure (`sdk.ts:318-327` @v0.83.0), whose `model` argument is the model
    /// of that very request.
    ///
    /// Fast path: the session's own resolved model, which is what every turn uses unless a
    /// `prepare_next_turn` hook retargeted the loop. Only a genuinely different model pays for a
    /// registry scan. `None` (an unknown model id) means "no opinion" and leaves the agent's static
    /// [`cyrup_agent::Agent::set_headers`] overlay in place.
    pub(super) fn headers_for_model_ref(&self, m: &ModelRef) -> Option<cyrup_provider::HeaderMap> {
        let matches = |c: &Model| c.provider == m.provider && c.id == m.model;
        let current = Self::lock(&self.compaction_model).clone();
        if let Some(cur) = current.as_ref().filter(|c| matches(c)) {
            return self.attribution_headers(cur);
        }
        self.full_model_registry().into_iter().find(matches).and_then(|c| self.attribution_headers(&c))
    }

    /// Emit the `model_select` extension event when the model actually changes (Pi `_emitModelSelect`,
    /// agent-session.ts:1429-1440). `source` ∈ `set`/`cycle`/`restore`. No-op when the model is
    /// unchanged (Pi `modelsAreEqual` guard).
    async fn emit_model_select(
        &self,
        next: &cyrup_provider::Model,
        previous: Option<&ModelRef>,
        source: &str,
    ) {
        // `modelsAreEqual`: same provider + id. Pi's `previousModel` is `Model | undefined`
        // (agent-session.ts:1559-1571) and `modelsAreEqual(undefined, next)` is false, so the FIRST
        // `/model` on a modelless session emits with `previousModel: undefined`.
        if previous.is_some_and(|p| p.provider == next.provider && p.model == next.id) {
            return;
        }
        let cancel = self.session_cancel.child_token();
        // EXT-042: `model`, `previousModel` and `source` are THREE SIBLING fields on pi's
        // `ModelSelectEvent` (`core/extensions/types.ts:792-799` @v0.83.0), emitted as such at
        // `agent-session.ts:1565-1570`. They used to be nested INSIDE `model` here, so a ported
        // handler read `event.previousModel` and got `undefined`.
        let model_val = serde_json::json!({
            "provider": next.provider.as_str(),
            "id": next.id.as_str(),
        });
        let previous_model = previous.map(|p| {
            serde_json::json!({ "provider": p.provider.as_str(), "id": p.model.as_str() })
        });
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::ModelSelect {
                    model: model_val,
                    previous_model,
                    source: source.to_string(),
                },
                &cancel,
            )
            .await;
    }

    /// Set the active model directly from a provider+id pair (no pattern matching).
    pub async fn set_model_id(
        &self,
        provider: ProviderId,
        model: ModelId,
    ) -> Result<(), SessionServiceError> {
        let model_ref = ModelRef { provider: provider.clone(), api: None, model: model.clone() };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = Some(model_ref.clone());
        self.bash_session_env.set_model(provider.to_string(), model.to_string());
        // Same per-request attribution rule as `apply_model_change` (pi `sdk.ts:318-327`). This
        // path has only the `ModelRef`, so resolve the full `Model` to recompute; if it cannot be
        // resolved the overlay is cleared rather than left stale — sending the OLD provider's
        // attribution is worse than sending none.
        let resolved = self
            .provider
            .current()
            .models()
            .iter()
            .find(|m| m.id == model_ref.model && m.provider == model_ref.provider)
            .cloned();
        self.agent
            .set_headers(resolved.as_ref().and_then(|m| self.attribution_headers(m)))
            .await;
        self.manager.lock().await.append_model_change(provider, model)?;
        Ok(())
    }

    /// The models available for `cycle_model` (Pi `scopedModels` getter, agent-session.ts:870).
    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        Self::lock(&self.scoped_models).clone()
    }

    /// Replace the scoped-model cycle set (Pi `setScopedModels`, agent-session.ts:875).
    ///
    /// Also re-seeds the guest-visible mirror behind `ctx.scopedModels` (EXT-045). pi exposes the
    /// SAME field to extensions off the base context — `getScopedModels: () => this._scopedModels`
    /// (`core/agent-session.ts:2416`), read by `get scopedModels()`
    /// (`core/extensions/runner.ts:706-709`) — so the extension seam and the `/scoped-models`
    /// command must never disagree. `LiveHostServices`'s read is SYNC and this set lives behind a
    /// lock it does not own, so it is mirrored the way the system prompt already is; doing it HERE,
    /// in the single writer, is what keeps the mirror from lagging.
    pub fn set_scoped_models(&self, models: Vec<ScopedModel>) {
        // pi's `ScopedModel` is `{model: Model<Api>; thinkingLevel?: ThinkingLevel}`
        // (`core/model-resolver.ts:63-67` @v0.83.0). `thinkingLevel` is OMITTED when unset, exactly
        // as an `undefined` field is absent from upstream's object.
        let mirrored: Vec<serde_json::Value> = models
            .iter()
            .map(|sm| {
                let mut obj = serde_json::Map::new();
                if let Ok(model) = serde_json::to_value(&sm.model) {
                    obj.insert("model".to_string(), model);
                }
                if let Some(level) = sm.thinking_level
                    && let Ok(level) = serde_json::to_value(level)
                {
                    obj.insert("thinkingLevel".to_string(), level);
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        self.services.host_services.update_scoped_models(mirrored);
        *Self::lock(&self.scoped_models) = models;
    }

    /// Cycle to the next/previous model (Pi `cycleModel`, agent-session.ts:1601-1671). Cycles over
    /// the scoped set when one is configured (filtered to models with configured auth), else the
    /// AUTH-FILTERED registry ([`Self::available_model_catalog`]). Returns a typed
    /// [`ModelCycleResult`] distinguishing the scoped vs available
    /// path + the restored thinking level, or `None` when there is one-or-fewer candidate. Applies
    /// the model + re-clamps/restores the thinking level, persists a `model_change`, and emits
    /// `model_changed` + the `model_select` ext event.
    pub async fn cycle_model(
        &self,
        forward: bool,
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let scoped = Self::lock(&self.scoped_models).clone();
        if scoped.is_empty() {
            self.cycle_available_model(forward).await
        } else {
            self.cycle_scoped_model(forward, &scoped).await
        }
    }

    /// Cycle over the scoped set, honoring per-model thinking levels (Pi `_cycleScopedModel`,
    /// agent-session.ts:1608-1641).
    async fn cycle_scoped_model(
        &self,
        forward: bool,
        scoped: &[ScopedModel],
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let candidates: Vec<&ScopedModel> =
            scoped.iter().filter(|s| self.has_configured_auth(&s.model)).collect();
        if candidates.len() <= 1 {
            return Ok(None);
        }
        let current = Self::lock(&self.model).clone();
        let cur_idx = candidates
            .iter()
            // pi `modelsAreEqual(sm.model, currentModel)` with `currentModel: Model | undefined`
            // never matches when the session is modelless, so `findIndex` returns -1 and pi's
            // `if (currentIndex === -1) currentIndex = 0` starts the cycle at the head
            // (agent-session.ts:1618-1621; the same shape at :1647-1650 for the available set).
            .position(|s| {
                current.as_ref().is_some_and(|c| {
                    s.model.provider == c.provider && s.model.id == c.model
                })
            })
            .unwrap_or(0);
        let len = candidates.len();
        let next_idx = if forward { (cur_idx + 1) % len } else { (cur_idx + len - 1) % len };
        let Some(next) = candidates.get(next_idx).copied() else {
            return Ok(None);
        };
        // Explicit scoped thinking level overrides; `None` inherits the current session level.
        let explicit = next.thinking_level;
        self.install_owning_provider(&next.model)?;
        let new_level = self
            .apply_model_change(&next.model, current.as_ref(), "cycle", explicit)
            .await?;
        Ok(Some(ModelCycleResult { model: next.model.clone(), thinking_level: new_level, is_scoped: true }))
    }

    /// Cycle over the AUTH-FILTERED registry (Pi `_cycleAvailableModel`, agent-session.ts:1643-1670,
    /// whose first line is `const availableModels = await this._modelRuntime.getAvailable()`).
    ///
    /// `getAvailable()` is `getAll().filter(hasConfiguredAuth)` across EVERY provider
    /// (model-runtime.ts:315-329 → models.ts:394-409), which is exactly
    /// [`Self::available_model_catalog`]. cyrup previously cycled `provider.current().models()` —
    /// the ONE installed provider's own catalog — so a user with `ANTHROPIC_API_KEY` and
    /// `OPENAI_API_KEY` could never cycle off the provider they launched on, the same SEAM-004 bug
    /// already fixed for `set_model`/`get_available_models` on the RPC seam.
    async fn cycle_available_model(
        &self,
        forward: bool,
    ) -> Result<Option<ModelCycleResult>, SessionServiceError> {
        let candidates = self.available_model_catalog();
        if candidates.len() <= 1 {
            return Ok(None);
        }
        let current = Self::lock(&self.model).clone();
        let cur_idx = candidates
            .iter()
            // Same `findIndex → -1 → 0` fallback as the scoped path (agent-session.ts:1647-1650).
            .position(|m| {
                current.as_ref().is_some_and(|c| m.provider == c.provider && m.id == c.model)
            })
            .unwrap_or(0);
        let len = candidates.len();
        let next_idx = if forward { (cur_idx + 1) % len } else { (cur_idx + len - 1) % len };
        let Some(next) = candidates.get(next_idx).cloned() else {
            return Ok(None);
        };
        self.install_owning_provider(&next)?;
        let new_level = self.apply_model_change(&next, current.as_ref(), "cycle", None).await?;
        Ok(Some(ModelCycleResult { model: next, thinking_level: new_level, is_scoped: false }))
    }

    /// Apply a resolved model change: push to the agent, re-derive headers, persist, re-clamp/restore
    /// the thinking level, emit `model_changed` + `model_select`. Returns the new thinking level.
    /// Shared by [`Self::set_model_resolved`] and the cycle paths.
    async fn apply_model_change(
        &self,
        next: &Model,
        previous: Option<&ModelRef>,
        source: &str,
        explicit_thinking: Option<ModelThinkingLevel>,
    ) -> Result<ModelThinkingLevel, SessionServiceError> {
        let model_ref = ModelRef {
            provider: next.provider.clone(),
            api: Some(next.api.clone()),
            model: next.id.clone(),
        };
        self.agent.set_model(model_ref.clone()).await;
        *Self::lock(&self.model) = Some(model_ref.clone());
        *Self::lock(&self.compaction_model) = Some(next.clone());
        self.services.host_services.update_model(model_ref, next.context_window, None);
        // Republish `CYRUP_PROVIDER`/`CYRUP_MODEL` for the NEXT `bash` child (Pi re-reads `ctx.model`
        // on every `resolveSpawnContext`, bash.ts:175-178; docs/environment-variables.md:27).
        self.bash_session_env.set_model(next.provider.to_string(), next.id.to_string());
        // ...and re-push the new model's image capability for the same reason: `read`'s
        // non-vision warning must describe the model the NEXT read will actually run against,
        // not the one resolved at startup.
        self.read_model_vision.set(next.supports_image_input());
        // pi recomputes provider-attribution + opencode session-affinity headers INSIDE `streamFn`,
        // dispatched on the model the request is actually going to (`sdk.ts:318-327`). cyrup merged
        // them once at session build and pinned them via `AgentBuilder::headers`, so a
        // cross-provider `/model` switch kept sending the PREVIOUS provider's attribution — an
        // OpenRouter `HTTP-Referer`/`X-Title` on an Anthropic request, or a stale opencode
        // session-affinity header. `attribution_headers()` existed and computed this correctly
        // per-model; it simply had no caller.
        self.agent.set_headers(self.attribution_headers(next)).await;
        self.manager.lock().await.append_model_change(next.provider.clone(), next.id.clone())?;
        // Re-clamp the thinking level for the new model (explicit override or current session level).
        let level = match explicit_thinking {
            Some(l) => l,
            None => self.thinking_level().await,
        };
        let new_level = self.set_thinking_level(level).await?;
        self.fanout_emit(AgentSessionEvent::ModelChanged {
            provider: next.provider.to_string(),
            model: next.id.to_string(),
        })
        .await;
        self.emit_model_select(next, previous, source).await;
        Ok(new_level)
    }
}

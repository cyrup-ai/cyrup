//! The **catalog layer** of virtual models: the catalog entry and the "not routed" refusal text.
//!
//! A **virtual model** is a catalog entry that routes each request to a physical model. The user
//! selects one model (`router/auto`); a router picks the physical model and thinking level per
//! request. Everything below the routing step only ever sees physical models: providers stream
//! them and assistant messages record them. A virtual model never reaches a provider.
//!
//! Ported from pi `packages/coding-agent/src/core/virtual-models.ts`, read at **v1.0.4** through
//! git objects (byte-identical at v1.0.0, v1.0.1 and v1.0.4 — 238 lines at each), plus
//! `packages/ai/src/api/lazy.ts` for the failure arm of `lazyStream`.
//!
//! # What is here, and what is not
//!
//! Here: [`VIRTUAL_MODEL_API`], [`is_virtual_model`] / [`is_virtual_any`],
//! [`create_virtual_model`] (pi `createVirtualModel`, `:169-187`) and [`unrouted_message`]
//! (pi `unroutedStream`'s text, `:189-194`).
//!
//! Not here, deliberately:
//!
//! - The **registry and the routing step** (pi `ModelRuntime.registerVirtualModel` /
//!   `resolveModel` / `getPhysicalModel`, `model-runtime.ts:947-1031`). A
//!   [`VirtualModelSpec`] carries no `route` callback for that reason — the registry composes
//!   `{ spec, route }` over it.
//! - The **branch selection** and the **router state entry** (pi `getBranchSelection` /
//!   `getVirtualModelState`, `:128-167`), which are already ported in
//!   `cyrup_session::virtual_models` (SESS-067).
//! - A port of the **`withVirtualModels` provider decorator** (`:196-238`). It once lived here and
//!   was never reachable; [`unrouted_message`]'s own doc records what it was, what replaces each
//!   of its three effects in production, and why wiring it in was the wrong answer.

use crate::api::compat::thinking_level_key;
use crate::classifier::AnyModel;
use crate::collection::EXTENDED_THINKING_LEVELS;
use crate::model::{Modality, Model, ModelCost, ThinkingLevelMap};
use cyrup_core::{ApiId, ModelId, ModelThinkingLevel, ProviderId};

/// Api id of virtual catalog entries — pi `VIRTUAL_MODEL_API` (`virtual-models.ts:29`).
///
/// Re-exported from [`cyrup_core`], which is the one definition the whole workspace shares; see
/// [`cyrup_core::VIRTUAL_MODEL_API`] for why it lives there.
pub use cyrup_core::VIRTUAL_MODEL_API;

/// pi `createVirtualModel`'s default thinking ladder: `definition.thinkingLevels ?? ["off"]`
/// (`virtual-models.ts:171`).
const DEFAULT_THINKING_LEVELS: [ModelThinkingLevel; 1] = [ModelThinkingLevel::Off];

/// Everything a virtual catalog entry declares — pi
/// `Omit<VirtualModelDefinition, "route">` (`virtual-models.ts:84-102`).
///
/// Every optional pi member stays an [`Option`] so the default is applied inside
/// [`create_virtual_model`] rather than at the caller, and so an explicitly EMPTY
/// `thinking_levels` stays empty rather than collapsing to pi's `["off"]` default.
///
/// It carries **no route callback**: that belongs to the registry, which composes its own
/// `{ spec, route }` definition over this.
///
/// # Why it is serde, in pi's camelCase
///
/// The WASM/guest extension tier registers through `registration.register-virtual-model(spec-json)`
/// (`cyrup-ext-sdk/wit/world.wit`), which carries this type as JSON because ADR-0002 keeps the
/// router callable off the value seam. The key names are therefore upstream's own — pi's
/// `Omit<VirtualModelDefinition, "route">` (`virtual-models.ts:84-102`), which an
/// `ExtensionVirtualModel` extends (`core/extensions/types.ts:1886-1889`) — so a guest author
/// writes the same object a pi extension author does. Every optional field is
/// `#[serde(default, skip_serializing_if = "Option::is_none")]`, which keeps the absent/`None`
/// distinction the doc above depends on: an OMITTED `thinkingLevels` stays `None` and picks up
/// [`create_virtual_model`]'s `["off"]` default, while an explicitly empty one stays empty.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualModelSpec {
    /// pi `provider` — the provider id the virtual model is listed under. MAY be a provider that
    /// also has physical models.
    pub provider: ProviderId,
    /// pi `id` — must not be the id of a physical model of `provider` (the registry enforces that;
    /// the decorator's shadowing rule covers a model a later catalog refresh adds).
    pub id: ModelId,
    /// pi `name`.
    pub name: String,
    /// pi `thinkingLevels?` — levels offered for selection. `None` defaults to `[Off]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_levels: Option<Vec<ModelThinkingLevel>>,
    /// pi `contextWindow?` — shown before the first response; afterwards the limits of the physical
    /// model that answered apply. `None` defaults to 0 ("unknown").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// pi `maxTokens?`. `None` defaults to 0 ("unknown").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// pi `input?` — input types accepted for selection. `None` defaults to text + image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<Modality>>,
}

/// Whether a chat model names a virtual model — pi `isVirtualModel` (`virtual-models.ts:105-107`).
#[must_use]
pub fn is_virtual_model(model: &Model) -> bool {
    model.api.as_str() == VIRTUAL_MODEL_API
}

/// Whether a catalog entry of ANY type names a virtual model.
///
/// pi's `isVirtualModel(model: { api: string })` is STRUCTURAL, so one function serves both the
/// typed-chat and the `AnyModel` legs of `withVirtualModels` (`:218-220`). cyrup's
/// [`AnyModel`] is an enum, so this second entry point is the port of that structural typing — not
/// an addition.
#[must_use]
pub fn is_virtual_any(model: &AnyModel) -> bool {
    model.api().as_str() == VIRTUAL_MODEL_API
}

/// Build the catalog entry of a virtual model — pi `createVirtualModel`
/// (`virtual-models.ts:169-187`).
///
/// # The thinking-level map holds ALL SEVEN keys
///
/// pi writes `thinkingLevelMap[level] = levels.includes(level) ? level : null` for every rung of
/// its ladder (`:173`), i.e. an offered level maps to its own NAME and an unoffered one to an
/// explicit `null`. The explicit null is the load-bearing half: cyrup's
/// [`crate::get_supported_thinking_levels`] reads an ABSENT key as SUPPORTED for every rung except
/// `xhigh`/`max`, so a map holding only `{low, high}` would offer `minimal` and `medium` on a
/// `thinking_levels: [Low, High]` virtual model. With all seven present it returns exactly
/// `[Low, High]`, which is what upstream's own test pins
/// (`getSupportedThinkingLevels(virtual)` ⇒ `["low","high"]`, `test/virtual-models.test.ts:122`).
///
/// cyrup's [`ThinkingLevelMap`] is `BTreeMap<String, Option<String>>`, which encodes pi's
/// three-way `Partial<Record<ModelThinkingLevel, string | null>>` (`ai/src/types.ts:87`) as
/// absent / `Some(Some(v))` / `Some(None)`.
///
/// Fields with no cyrup counterpart are not dropped: pi's entry declares no `promptCache`,
/// `samplingParamsByThinkingLevel`, `inputLimits` or `type: "chat"`, so `sampling_params`,
/// `compat` and `headers` are `None` and the entry is a chat [`Model`] by type.
#[must_use]
pub fn create_virtual_model(spec: &VirtualModelSpec) -> Model {
    // `const levels = definition.thinkingLevels ?? ["off"]` (`:171`).
    let levels: &[ModelThinkingLevel] = spec
        .thinking_levels
        .as_deref()
        .unwrap_or(&DEFAULT_THINKING_LEVELS);
    // `for (const level of THINKING_LEVELS) thinkingLevelMap[level] = levels.includes(level) ?
    // level : null` (`:172-173`) — every rung, offered ⇒ its own name, unoffered ⇒ explicit null.
    let mut thinking_level_map = ThinkingLevelMap::new();
    for level in EXTENDED_THINKING_LEVELS {
        let key = thinking_level_key(level);
        thinking_level_map.insert(
            key.to_string(),
            levels.contains(&level).then(|| key.to_string()),
        );
    }
    Model {
        id: spec.id.clone(),
        name: spec.name.clone(),
        api: ApiId::from(VIRTUAL_MODEL_API),
        provider: spec.provider.clone(),
        // `baseUrl: ""` (`:179`) — a virtual model has no endpoint, because it is never streamed.
        base_url: String::new(),
        // `reasoning: levels.some((level) => level !== "off")` (`:180`).
        reasoning: levels.iter().any(|level| *level != ModelThinkingLevel::Off),
        // `input: definition.input ?? ["text", "image"]` (`:182`).
        input: spec
            .input
            .clone()
            .unwrap_or_else(|| vec![Modality::Text, Modality::Image]),
        // `cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }` (`:183`) plus pi's absent
        // tier ladder, which is exactly `ModelCost::default()`.
        cost: ModelCost::default(),
        // `contextWindow: definition.contextWindow ?? 0`, `maxTokens: definition.maxTokens ?? 0`
        // (`:184-185`) — "unset limits are unknown (0)".
        context_window: spec.context_window.unwrap_or(0),
        max_tokens: spec.max_tokens.unwrap_or(0),
        sampling_params: None,
        thinking_level_map: Some(thinking_level_map),
        compat: None,
        headers: None,
    }
}

/// The error text of an unrouted virtual-model request — pi `unroutedStream`
/// (`virtual-models.ts:191`). Upstream's own test asserts on this substring
/// (`test/virtual-models.test.ts:260`), so there must be exactly one spelling of it in the tree.
///
/// # Where the refusal is PRODUCED
///
/// `[CYRUP-DELTA]`, recorded because it is a structural divergence and not an omission. Upstream
/// has exactly one producer — the `withVirtualModels` decorator's `unroutedStream`
/// (`virtual-models.ts:189-194`, reached from `:233-235`) — because every request in pi goes
/// through a `Provider` object and `ModelRuntime` composes each provider through that decorator,
/// so wrapping the provider IS the guard.
///
/// cyrup installs exactly ONE provider at a time and merges the virtual rows into a flat
/// `Vec<Model>` catalog instead ([`crate::VirtualModelRegistry::apply_to_catalog`], applied by
/// `AgentSession::compose_model_registry` and by the session builder's `resolve_model`), so there
/// is no decorated provider to carry the guard and the two paths that can be handed a selection
/// carry it themselves:
///
/// * the agent loop, at `cyrup_agent::ProviderStreamFn::stream`
///   (`crates/cyrup-agent/src/stream_fn.rs`);
/// * the extension-facing standalone completion, at `LiveHostServices::complete_standalone`
///   (`crates/cyrup-session-svc/src/host_services.rs`), which is pi's
///   `ctx.modelRegistry.complete` (`core/model-registry.ts:141`) — and `complete` is
///   `stream().result()` with no virtual arm of its own (`model-runtime.ts:691-712`), so upstream
///   reaches the decorator there too.
///
/// Both answer with this text.
///
/// # What was deleted, and why it was not wired in instead
///
/// A `VirtualModelProvider` decorator, a `with_virtual_models` constructor and a
/// `VirtualModelRegistry::wrap_provider` once lived here (~360 lines plus 18 tests) as the literal
/// port of `withVirtualModels`. They had ZERO production callers at any point in their life: the
/// catalog merge was always `apply_to_catalog`, virtual-only availability was always
/// `AgentSession::provider_is_available`
/// (`crates/cyrup-session-svc/src/session/model.rs:141-153`), and the refusal was always the two
/// sites above. Wiring them in would mean INSTALLING a provider for a provider id that has no
/// credentials and no physical models, which `AgentSession::set_model_resolved` deliberately
/// refuses to do for a virtual model (`model.rs:60-67`, with its own reasoning: a virtual-only
/// provider id has no entry in the bin's `ProviderResolver` at all, so `resolve_and_store` would
/// fail with `NoConfiguredAuth` and make the selection impossible). So they are deleted.
///
/// Takes the pair rather than a `&Model` because one of the two callers has only a
/// [`cyrup_core::ModelRef`].
#[must_use]
pub fn unrouted_message(provider: &str, id: &str) -> String {
    format!("Virtual model {provider}/{id} must be routed before streaming")
}

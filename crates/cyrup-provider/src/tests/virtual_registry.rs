//! The virtual-model REGISTRY and the ROUTING STEP (`crate::virtual_registry`): pi
//! `registerVirtualModel` / `unregisterVirtualModel` (`model-runtime.ts:947-982`), `resolveModel`
//! (`:984-1024`), `getPhysicalModel` (`:1026-1030`) and `findLatestResponse`
//! (`virtual-models.ts:109-118`), read at **v1.0.4** through git objects.
//!
//! The fixture is upstream's own (`packages/coding-agent/test/virtual-models.test.ts:27-46`): a
//! `faux` provider listing `small` (context 1000 / max 100 / text, no reasoning) and `large`
//! (context 50 000 / max 5000 / text+image, reasoning), and a virtual `router/auto` offering
//! `["low", "high"]` whose router sends `high` to `large` and anything else to `small` while always
//! ASKING for `high` — which is what makes the clamp observable.
//!
//! Every test below names the behaviour it pins and was red-proven by removing that behaviour
//! first; the ones that cannot fail that way say so in their own doc comment and are regression
//! guards, not proofs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use crate::collection::get_supported_thinking_levels;
use crate::model::{Modality, Model, ModelCost};
use crate::virtual_models::{VIRTUAL_MODEL_API, VirtualModelSpec, create_virtual_model};
use crate::virtual_registry::{
    FailedRequest, ModelRoute, ModelRouteError, ModelRouteReason, ModelRouteRequest, ModelRouter,
    NoCatalog, RouteOptions, RoutedModel, VirtualModelCatalog, VirtualModelDefinition,
    VirtualModelError, VirtualModelListener, VirtualModelRegistry, find_latest_response,
};
use cyrup_core::{AssistantMessage, Message, ModelThinkingLevel, ProviderId, StopReason};
use serde_json::{Value, json};

// ---------------------------------------------------------------- fixtures ----

/// Upstream's faux rows. `reasoning` is what decides the clamp, so it is explicit.
fn chat(provider: &str, id: &str, context_window: u64, max_tokens: u64, reasoning: bool) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        api: "openai-completions".into(),
        provider: provider.into(),
        base_url: "http://faux.test/v1".into(),
        reasoning,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        input_limits: None,
        prompt_cache: None,
        context_window,
        max_tokens,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn small() -> Model {
    chat("faux", "small", 1000, 100, false)
}

fn large() -> Model {
    chat("faux", "large", 50_000, 5000, true)
}

fn spec(provider: &str, id: &str) -> VirtualModelSpec {
    VirtualModelSpec {
        provider: provider.into(),
        id: id.into(),
        name: "Auto".to_string(),
        thinking_levels: Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]),
        context_window: None,
        max_tokens: None,
        input: None,
    }
}

fn router_auto() -> Model {
    create_virtual_model(&spec("router", "auto"))
}

fn ids(models: &[Model]) -> Vec<String> {
    models.iter().map(|m| m.id.as_str().to_string()).collect()
}

fn names(providers: &[ProviderId]) -> Vec<String> {
    providers.iter().map(|p| p.as_str().to_string()).collect()
}

/// A settled assistant turn naming `model`, i.e. one `find_latest_response` must return.
fn answered(model: &Model, stop_reason: StopReason) -> Message {
    let mut a = AssistantMessage::errored(
        model.provider.clone(),
        model.id.as_str(),
        Some(model.api.clone()),
        stop_reason,
        "",
    );
    a.error_message = None;
    Message::Assistant(a)
}

/// A failed turn, with an error message a router can read — upstream's `"overloaded_error"`.
fn failure(model: &Model, text: &str) -> AssistantMessage {
    AssistantMessage::errored(
        model.provider.clone(),
        model.id.as_str(),
        Some(model.api.clone()),
        StopReason::Error,
        text,
    )
}

fn user(text: &str) -> Message {
    Message::User {
        content: vec![cyrup_core::Content::Text {
            text: text.into(),
            text_signature: None,
        }],
        timestamp: 1,
    }
}

// ------------------------------------------------------------- catalog double ----

/// A [`VirtualModelCatalog`] over a fixed model list, an explicit configured-provider set, and an
/// explicit physical-provider set — the three reads pi takes off `ModelRuntime`.
///
/// `models` deliberately holds VIRTUAL rows too when a test needs them, because the registry's
/// conflict check must distinguish a replacement from a conflict.
#[derive(Default)]
struct Catalog {
    models: Vec<Model>,
    configured: Vec<String>,
    physical_providers: Vec<String>,
}

impl Catalog {
    /// The default fixture: `faux` with both rows, configured and physically present.
    fn faux() -> Self {
        Self {
            models: vec![small(), large()],
            configured: vec!["faux".to_string()],
            physical_providers: vec!["faux".to_string()],
        }
    }

    fn empty() -> Self {
        Self::default()
    }

    fn with(mut self, model: Model) -> Self {
        self.models.push(model);
        self
    }

    fn unconfigured(mut self) -> Self {
        self.configured.clear();
        self
    }
}

impl VirtualModelCatalog for Catalog {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.models
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
            .cloned()
    }
    fn models(&self) -> Vec<Model> {
        self.models.clone()
    }
    fn has_configured_auth(&self, provider: &str) -> bool {
        self.configured.iter().any(|p| p == provider)
    }
    fn has_physical_provider(&self, provider: &str) -> bool {
        self.physical_providers.iter().any(|p| p == provider)
    }
}

// -------------------------------------------------------------- router doubles ----

/// What a [`Recorder`] router saw, flattened so an assertion names only what it cares about.
#[derive(Default)]
struct Seen {
    reasons: Vec<ModelRouteReason>,
    thinking_levels: Vec<ModelThinkingLevel>,
    previous: Vec<Option<RoutedModel>>,
    failed: Vec<Option<FailedRequest>>,
    states: Vec<Option<Value>>,
    message_counts: Vec<usize>,
}

/// Upstream's own router: `high` goes to `large`, anything else to `small`, and it always ASKS for
/// `high` (`test/virtual-models.test.ts:39-43`). Records every request it was handed.
struct Recorder {
    seen: Arc<Mutex<Seen>>,
    /// What to answer. `None` means upstream's small/large pick.
    answer: Option<Model>,
    /// State to return, if any.
    state: Option<Value>,
}

impl Recorder {
    fn new() -> (Arc<Self>, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        (
            Arc::new(Self {
                seen: seen.clone(),
                answer: None,
                state: None,
            }),
            seen,
        )
    }

    fn answering(model: Model) -> Arc<Self> {
        Arc::new(Self {
            seen: Arc::new(Mutex::new(Seen::default())),
            answer: Some(model),
            state: None,
        })
    }

    fn returning_state(state: Value) -> Arc<Self> {
        Arc::new(Self {
            seen: Arc::new(Mutex::new(Seen::default())),
            answer: None,
            state: Some(state),
        })
    }
}

#[async_trait::async_trait]
impl ModelRouter for Recorder {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        if let Ok(mut g) = self.seen.lock() {
            g.reasons.push(request.reason);
            g.thinking_levels.push(request.thinking_level);
            g.previous.push(request.previous.clone());
            g.failed.push(request.failed.clone());
            g.states.push(request.state.cloned());
            g.message_counts.push(request.messages.len());
        }
        let model = self.answer.clone().unwrap_or_else(|| {
            if request.thinking_level == ModelThinkingLevel::High {
                large()
            } else {
                small()
            }
        });
        Ok(ModelRoute {
            model,
            // Always ask for `high`: the clamp is only observable when the router over-asks.
            thinking_level: ModelThinkingLevel::High,
            state: self.state.clone(),
        })
    }
}

/// A router that fails, which is pi's `route()` throwing.
struct Failing(&'static str);

#[async_trait::async_trait]
impl ModelRouter for Failing {
    async fn route(&self, _request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        Err(ModelRouteError::new(self.0))
    }
}

fn definition(provider: &str, id: &str, router: Arc<dyn ModelRouter>) -> VirtualModelDefinition {
    VirtualModelDefinition::new(spec(provider, id), router)
}

/// A registry with `router/auto` registered against the faux catalog.
fn registry_with(router: Arc<dyn ModelRouter>) -> VirtualModelRegistry {
    let registry = VirtualModelRegistry::new();
    registry
        .register(definition("router", "auto", router), &Catalog::faux())
        .expect("router/auto registers under an id no physical model holds");
    registry
}

fn route_opts(level: ModelThinkingLevel) -> RouteOptions<'static> {
    RouteOptions::new(ModelRouteReason::User, level)
}

// ------------------------------------------------------ find_latest_response ----

/// pi `findLatestResponse` (`virtual-models.ts:110-116`) skips `error` and `aborted`, which is the
/// whole reason the field is safe to feed `previous`. RED-PROVEN by dropping the `Error` arm of the
/// skip: the trailing failure is returned and the assertion on `small` fails.
#[test]
fn find_latest_response_skips_failed_and_aborted_responses() {
    let messages = vec![
        user("first"),
        answered(&large(), StopReason::Stop),
        answered(&small(), StopReason::Stop),
        answered(&large(), StopReason::Aborted),
        answered(&large(), StopReason::Error),
    ];
    let latest = find_latest_response(&messages).expect("one settled response remains");
    assert_eq!(latest.model, "small");
    assert_eq!(latest.stop_reason, StopReason::Stop);
}

/// A transcript whose only assistant turn came from FAILED ROUTING names the virtual model and is
/// an error, so it is skipped — leaving nothing. This is what makes
/// `resolve_model`'s `previous` absent after a routing failure rather than wrong.
/// RED-PROVEN with the same neuter as above: the virtual-named failure is returned.
#[test]
fn find_latest_response_skips_an_assistant_message_left_by_failed_routing() {
    let virtual_model = router_auto();
    let messages = vec![user("hi"), answered(&virtual_model, StopReason::Error)];
    assert!(find_latest_response(&messages).is_none());
}

/// REGRESSION GUARD, not a red proof: it passes with the skip removed too, because there is no
/// assistant message at all. It pins that a tool-result tail does not end the walk early.
#[test]
fn find_latest_response_is_none_without_any_response() {
    assert!(find_latest_response(&[user("hi")]).is_none());
    assert!(find_latest_response(&[]).is_none());
}

// -------------------------------------------------------------- registration ----

/// pi `if (!providerId.trim() || !id.trim()) throw new Error("Virtual model provider and id must
/// not be empty.")` (`model-runtime.ts:956`), with the exact text. RED-PROVEN by deleting the trim
/// guard: both registrations succeed and `is_empty()` is false.
#[test]
fn register_rejects_an_empty_provider_or_id() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    for (provider, id) in [
        ("", "auto"),
        ("router", ""),
        ("  ", "auto"),
        ("router", " "),
    ] {
        let err = registry
            .register(definition(provider, id, router.clone()), &NoCatalog)
            .expect_err("an empty or blank provider or id is refused");
        assert_eq!(err, VirtualModelError::EmptyProviderOrId);
        assert_eq!(
            err.to_string(),
            "Virtual model provider and id must not be empty."
        );
    }
    assert!(registry.is_empty());
}

/// pi `const existing = this.models.getModel(providerId, id); if (existing &&
/// !isVirtualModel(existing)) throw` (`:957-960`), with the exact text upstream's own test asserts
/// a substring of (`test/virtual-models.test.ts:191`). RED-PROVEN by deleting the conflict check:
/// the registration succeeds and `faux/large` is shadowed by a virtual row.
#[test]
fn register_rejects_an_id_that_names_a_physical_model() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    let err = registry
        .register(definition("faux", "large", router), &Catalog::faux())
        .expect_err("faux/large is a physical model of faux");
    assert_eq!(
        err.to_string(),
        "Virtual model faux/large conflicts with a physical model."
    );
    assert!(registry.is_empty());
}

/// The conflict check reads `get_model`, NOT `physical_model`: an id that already names a VIRTUAL
/// model is a replacement, not a conflict (pi's `existing && !isVirtualModel(existing)`).
/// RED-PROVEN by testing against `physical_model` instead — the second registration then also
/// succeeds, so this test needs the catalog to already carry the virtual row to discriminate, and
/// by testing `existing.is_some()` alone, which makes it fail.
#[test]
fn register_replaces_a_virtual_model_already_in_the_catalog() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    // The catalog already lists router/auto as a virtual row, exactly as it would after a compose.
    let catalog = Catalog::empty().with(router_auto());
    registry
        .register(definition("router", "auto", router), &catalog)
        .expect("a virtual row of the same id is a replacement, not a conflict");
    assert!(registry.is_registered("router", "auto"));
}

/// pi `models.set(id, …)` replaces IN PLACE and keeps the key's insertion position
/// (`model-runtime.ts:962-964`), and the position is observable in the catalog listing.
/// RED-PROVEN two ways: pushing instead of replacing makes the listing `["auto","fast","auto"]`;
/// and routing afterwards reaches the FIRST router, so the `small` assertion fails.
#[tokio::test]
async fn re_registering_replaces_the_router_in_place() {
    let registry = VirtualModelRegistry::new();
    let (first, first_seen) = Recorder::new();
    registry
        .register(definition("router", "auto", first), &Catalog::empty())
        .unwrap();
    registry
        .register(
            definition("router", "fast", Recorder::answering(large())),
            &Catalog::empty(),
        )
        .unwrap();
    // Replace router/auto with a router that always answers `small`.
    registry
        .register(
            definition("router", "auto", Recorder::answering(small())),
            &Catalog::empty().with(router_auto()),
        )
        .unwrap();

    assert_eq!(ids(&registry.models_for("router")), ["auto", "fast"]);
    let route = registry
        .resolve_model(
            &router_auto(),
            &[],
            route_opts(ModelThinkingLevel::High),
            &Catalog::faux(),
        )
        .await
        .expect("the replacement router answers a physical model");
    assert_eq!(route.model.id.as_str(), "small");
    assert!(
        first_seen.lock().unwrap().reasons.is_empty(),
        "the replaced router must never be called again"
    );
}

/// pi `unregisterVirtualModel` (`:976-982`): an absent pair is a NO-OP, and removal is scoped to
/// one `(provider, id)` — upstream's own test unregisters `faux/fast` and `router/auto` and expects
/// `faux` to keep `["small","large","auto"]` and `router` to keep `["second"]`
/// (`test/virtual-models.test.ts:189-195`). RED-PROVEN two ways: keying the retain on the provider
/// alone wipes the sibling; returning `true` unconditionally fails the no-op assertion.
#[test]
fn unregister_removes_only_that_model_and_reports_whether_it_did() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    for (provider, id) in [("faux", "auto"), ("faux", "fast"), ("router", "auto")] {
        registry
            .register(definition(provider, id, router.clone()), &Catalog::faux())
            .unwrap();
    }
    assert!(
        !registry.unregister("router", "missing"),
        "absent is a no-op"
    );
    assert!(!registry.unregister("nobody", "auto"), "absent is a no-op");
    assert!(registry.unregister("faux", "fast"));
    assert_eq!(ids(&registry.models_for("faux")), ["auto"]);
    assert_eq!(ids(&registry.models_for("router")), ["auto"]);
    assert!(registry.unregister("router", "auto"));
    assert!(registry.models_for("router").is_empty());
    assert_eq!(names(&registry.provider_ids()), ["faux"]);
}

/// pi marks a provider of ONLY virtual models configured eagerly, because *"session restore checks
/// auth before the refresh below lands"* (`:962-968`). The discriminating pair is the next test.
/// RED-PROVEN by deleting the marking: `virtual_only_providers()` comes back empty.
#[test]
fn a_provider_only_virtual_models_define_is_marked_configured() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    registry
        .register(definition("router", "auto", router), &Catalog::faux())
        .unwrap();
    assert_eq!(names(&registry.virtual_only_providers()), ["router"]);
}

/// The other half: a virtual model listed under a provider that DOES have physical models inherits
/// that provider's own auth answer and must NOT be marked — pi's guard is
/// `!this.recomposeProvider(providerId)`. RED-PROVEN by dropping the `has_physical_provider`
/// conjunct: `faux` appears in the list, which would mark a credential-less real provider as
/// configured.
#[test]
fn a_provider_with_physical_models_is_not_marked_virtual_only() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    registry
        .register(definition("faux", "auto", router), &Catalog::faux())
        .unwrap();
    assert!(registry.virtual_only_providers().is_empty());
}

/// The marking goes when the provider's last virtual model does (pi `if (models.size === 0)
/// this.virtualModels.delete(providerId)`, `:978`), or a dead router id stays "configured" forever.
/// RED-PROVEN by dropping the `virtual_only.retain` from `unregister`: `router` survives.
#[test]
fn the_virtual_only_marking_goes_with_the_last_virtual_model() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    registry
        .register(
            definition("router", "auto", router.clone()),
            &Catalog::faux(),
        )
        .unwrap();
    registry
        .register(definition("router", "second", router), &Catalog::faux())
        .unwrap();
    registry.unregister("router", "auto");
    assert_eq!(names(&registry.virtual_only_providers()), ["router"]);
    registry.unregister("router", "second");
    assert!(registry.virtual_only_providers().is_empty());
}

// ------------------------------------------------------------ catalog overlay ----

/// Upstream pins `getModels("faux") == ["small","large","auto","fast"]` and
/// `getModels("router") == ["auto","second"]` (`test/virtual-models.test.ts:174-175`): physical rows
/// first, then virtual rows in REGISTRATION order, and a provider with no physical rows appends.
/// RED-PROVEN by holding the entries in a `BTreeMap<String, _>` keyed on the model id, which sorts
/// `fast` before `auto` on `faux` and fails both assertions.
#[test]
fn apply_to_catalog_appends_virtual_rows_in_registration_order() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    for (provider, id) in [
        ("faux", "auto"),
        ("faux", "fast"),
        ("router", "auto"),
        ("router", "second"),
    ] {
        registry
            .register(definition(provider, id, router.clone()), &Catalog::faux())
            .unwrap();
    }
    let mut catalog = vec![small(), large()];
    registry.apply_to_catalog(&mut catalog);
    let faux: Vec<String> = catalog
        .iter()
        .filter(|m| m.provider.as_str() == "faux")
        .map(|m| m.id.as_str().to_string())
        .collect();
    let router_rows: Vec<String> = catalog
        .iter()
        .filter(|m| m.provider.as_str() == "router")
        .map(|m| m.id.as_str().to_string())
        .collect();
    assert_eq!(faux, ["small", "large", "auto", "fast"]);
    assert_eq!(router_rows, ["auto", "second"]);
    // The faux rows stay contiguous and ahead of the router rows.
    assert_eq!(
        ids(&catalog),
        ["small", "large", "auto", "fast", "auto", "second"]
    );
}

/// Registration order is **not** id order, and only a container that preserves insertion can tell
/// them apart. Upstream's own fixture happens to register `auto` before `fast`, which is also
/// alphabetical, so its assertion cannot catch a sorted container — this one registers `zebra`
/// before `alpha` deliberately.
/// RED-PROVEN by sorting the entries by model id before the insert loop (the `BTreeMap<String, _>`
/// shape): the listing comes back `["small","large","alpha","zebra"]`.
#[test]
fn apply_to_catalog_does_not_sort_virtual_rows_by_id() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    for id in ["zebra", "alpha"] {
        registry
            .register(definition("faux", id, router.clone()), &Catalog::faux())
            .unwrap();
    }
    let mut catalog = vec![small(), large()];
    registry.apply_to_catalog(&mut catalog);
    assert_eq!(ids(&catalog), ["small", "large", "zebra", "alpha"]);
}

/// pi: *"A virtual model hides a physical chat model with the same id, which a catalog refresh can
/// add after registration"* (`virtual-models.ts:198-200`, filter at `:218`). A merge that keeps the
/// first occurrence inverts this, so the colliding row must be REMOVED.
/// RED-PROVEN by dropping the `retain`: the catalog holds TWO `faux/auto` rows with the physical
/// one first, so a lookup answers the physical row and the virtual model never routes.
#[test]
fn apply_to_catalog_hides_a_physical_row_a_refresh_added_under_a_virtual_id() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    registry
        .register(definition("faux", "auto", router), &Catalog::faux())
        .unwrap();
    // A later refresh added a PHYSICAL faux/auto, which is exactly the case upstream calls out.
    let mut catalog = vec![small(), chat("faux", "auto", 9, 9, false), large()];
    registry.apply_to_catalog(&mut catalog);
    assert_eq!(ids(&catalog), ["small", "large", "auto"]);
    let auto = catalog.iter().find(|m| m.id.as_str() == "auto").unwrap();
    assert_eq!(auto.api.as_str(), VIRTUAL_MODEL_API);
    assert_eq!(auto.context_window, 0, "the virtual row's own limits win");
}

/// REGRESSION GUARD, not a red proof: an empty registry must leave a catalog byte-identical, which
/// is the "a session with no virtual models behaves exactly as before" invariant.
#[test]
fn apply_to_catalog_is_a_no_op_with_nothing_registered() {
    let registry = VirtualModelRegistry::new();
    let mut catalog = vec![small(), large()];
    registry.apply_to_catalog(&mut catalog);
    assert_eq!(catalog, vec![small(), large()]);
}

// ----------------------------------------------------- generation + listener ----

/// `generation()` is the cache key a composed catalog must include, and the listener is pi's
/// `recomposeProvider` + `updateModelSnapshot` + `void refresh({ allowNetwork: false })` tail
/// (`:965-973`, `:980-982` — the cache-only refresh `SEAM-144` names).
/// RED-PROVEN two ways: dropping the `generation.fetch_add` leaves the key at 0 across a
/// registration AND an unregistration, so a cached catalog never invalidates; dropping the
/// `listener.virtual_models_changed` call leaves `notified` empty, so nothing ever re-wraps.
#[test]
fn registration_and_removal_bump_the_generation_and_notify_the_listener() {
    #[derive(Default)]
    struct Spy(Mutex<Vec<String>>);
    impl VirtualModelListener for Spy {
        fn virtual_models_changed(&self, provider: &ProviderId) {
            if let Ok(mut g) = self.0.lock() {
                g.push(provider.as_str().to_string());
            }
        }
    }
    let spy = Arc::new(Spy::default());
    let registry = VirtualModelRegistry::new();
    registry.set_listener(spy.clone());
    let (router, _) = Recorder::new();

    let start = registry.generation();
    registry
        .register(
            definition("router", "auto", router.clone()),
            &Catalog::faux(),
        )
        .unwrap();
    let after_register = registry.generation();
    assert!(after_register > start, "a registration moves the cache key");

    // A refused registration changes nothing: nothing was registered, so nothing must recompose.
    registry
        .register(definition("faux", "large", router), &Catalog::faux())
        .expect_err("conflicts with a physical model");
    assert_eq!(registry.generation(), after_register);

    // An absent unregister is a no-op, exactly as pi's `if (!models?.delete(id)) return;`.
    assert!(!registry.unregister("router", "missing"));
    assert_eq!(registry.generation(), after_register);

    assert!(registry.unregister("router", "auto"));
    assert!(registry.generation() > after_register);
    assert_eq!(*spy.0.lock().unwrap(), ["router", "router"]);
}

// --------------------------------------------------------------- resolve_model ----

/// pi `if (!virtual) throw new Error(`${name} is not registered.`)` (`:996-998`), with the exact
/// text. RED-PROVEN by removing the lookup guard and unwrapping instead: the call panics rather
/// than answering the user-facing message a routing failure must carry.
#[tokio::test]
async fn resolve_model_refuses_a_virtual_model_that_is_not_registered() {
    let registry = VirtualModelRegistry::new();
    let err = registry
        .resolve_model(
            &router_auto(),
            &[],
            route_opts(ModelThinkingLevel::Low),
            &Catalog::faux(),
        )
        .await
        .expect_err("nothing is registered");
    assert_eq!(
        err.to_string(),
        "Virtual model router/auto is not registered."
    );
}

/// Upstream's first assertion set (`test/virtual-models.test.ts:117-137`): the entry's own shape,
/// `getSupportedThinkingLevels == ["low","high"]`, then `low` routing to `small` whose level clamps
/// to `off` *"The router asked for 'high', but the small model does not reason"*, and `high`
/// routing to `large` at `high`.
/// RED-PROVEN by dropping the `clamp_thinking_level` call: the small route comes back at `high`.
#[tokio::test]
async fn resolve_model_routes_and_clamps_the_thinking_level_to_the_routed_model() {
    let virtual_model = router_auto();
    assert_eq!(virtual_model.provider.as_str(), "router");
    assert_eq!(virtual_model.context_window, 0);
    assert_eq!(virtual_model.max_tokens, 0);
    assert_eq!(
        get_supported_thinking_levels(&virtual_model),
        [ModelThinkingLevel::Low, ModelThinkingLevel::High]
    );
    let (router, seen) = Recorder::new();
    let registry = registry_with(router);
    let catalog = Catalog::faux();
    let messages = vec![
        user("first"),
        answered(&large(), StopReason::Stop),
        user("second"),
    ];

    let low = registry
        .resolve_model(
            &virtual_model,
            &messages,
            route_opts(ModelThinkingLevel::Low),
            &catalog,
        )
        .await
        .unwrap();
    assert_eq!(low.model.id.as_str(), "small");
    assert_eq!(low.thinking_level, ModelThinkingLevel::Off);

    let high = registry
        .resolve_model(
            &virtual_model,
            &messages,
            route_opts(ModelThinkingLevel::High),
            &catalog,
        )
        .await
        .unwrap();
    assert_eq!(high.model.id.as_str(), "large");
    assert_eq!(high.thinking_level, ModelThinkingLevel::High);
    // The SELECTED level reaches the router untouched; the clamp applies only to its answer.
    assert_eq!(
        seen.lock().unwrap().thinking_levels,
        [ModelThinkingLevel::Low, ModelThinkingLevel::High]
    );
}

/// The returned model is the CATALOG's row, not the router's object (pi
/// `const target = this.getPhysicalModel(route.model.provider, route.model.id)`, `:1019`), so a
/// router that hands back a stale or hand-built `Model` cannot smuggle wrong limits into the
/// request. RED-PROVEN by returning `route.model` instead of `target`: the window comes back 7.
#[tokio::test]
async fn resolve_model_returns_the_catalogs_row_not_the_routers_object() {
    let stale = chat("faux", "large", 7, 7, true);
    let registry = registry_with(Recorder::answering(stale));
    let route = registry
        .resolve_model(
            &router_auto(),
            &[],
            route_opts(ModelThinkingLevel::High),
            &Catalog::faux(),
        )
        .await
        .unwrap();
    assert_eq!(route.model.context_window, 50_000);
    assert_eq!(route.model.max_tokens, 5000);
}

/// pi builds `previous` from `findLatestResponse` mapped through `getPhysicalModel`
/// (`:1000-1001`), and upstream asserts `requests[0].previous == { model: large, thinkingLevel }`
/// (`test/virtual-models.test.ts:134`). RED-PROVEN two ways: building `previous` from the SELECTED
/// virtual model gives `router/auto`; taking the LAST assistant message rather than the last
/// SETTLED one gives `small` (the aborted turn).
#[tokio::test]
async fn resolve_model_reports_the_latest_successful_response_as_previous() {
    let (router, seen) = Recorder::new();
    let registry = registry_with(router);
    let messages = vec![
        user("first"),
        answered(&large(), StopReason::Stop),
        answered(&small(), StopReason::Aborted),
        user("second"),
    ];
    registry
        .resolve_model(
            &router_auto(),
            &messages,
            route_opts(ModelThinkingLevel::High),
            &Catalog::faux(),
        )
        .await
        .unwrap();
    let g = seen.lock().unwrap();
    let previous = g.previous[0].as_ref().expect("a settled response exists");
    assert_eq!(previous.model.id.as_str(), "large");
    assert_eq!(previous.model.context_window, 50_000);
    // The whole transcript reaches the router, system messages included (pi `messages`).
    assert_eq!(g.message_counts[0], 4);
}

/// A transcript whose latest response came from a provider the catalog no longer lists has NO
/// physical previous: pi's `latest && this.getPhysicalModel(...)` yields undefined.
/// RED-PROVEN by mapping through `get_model` instead of `physical_model`... which answers the same
/// here, so the discriminating neuter is dropping the `physical_model` map entirely and reporting
/// `latest`'s own address: `previous` then names `ghost/one`.
#[tokio::test]
async fn resolve_model_reports_no_previous_when_the_catalog_lost_that_model() {
    let (router, seen) = Recorder::new();
    let registry = registry_with(router);
    let gone = chat("ghost", "one", 10, 10, false);
    let messages = vec![user("hi"), answered(&gone, StopReason::Stop)];
    registry
        .resolve_model(
            &router_auto(),
            &messages,
            route_opts(ModelThinkingLevel::High),
            &Catalog::faux(),
        )
        .await
        .unwrap();
    assert!(seen.lock().unwrap().previous[0].is_none());
}

/// Upstream: *"A routing failure names the virtual model, so there is no failed physical request to
/// report"* — `requests[0].failed == { model: large, …, message: failed }` and
/// `requests[1].failed` is undefined (`test/virtual-models.test.ts:157-163`). `failed` and
/// `previous` are independent: the retry's failed request is `large` while the latest SUCCESSFUL
/// response is `small`.
/// RED-PROVEN by not mapping `failed` through `physical_model`: the virtual-named failure is
/// reported as a failed physical request, and the second assertion fails.
#[tokio::test]
async fn resolve_model_reports_the_failed_request_separately_and_drops_a_virtual_one() {
    let (router, seen) = Recorder::new();
    let registry = registry_with(router);
    let catalog = Catalog::faux();
    let messages = vec![
        user("first"),
        answered(&small(), StopReason::Stop),
        user("second"),
    ];
    let failed_large = failure(&large(), "overloaded_error");
    let failed_routing = failure(&router_auto(), "boom");

    for failed in [&failed_large, &failed_routing] {
        registry
            .resolve_model(
                &router_auto(),
                &messages,
                RouteOptions {
                    failed: Some(failed),
                    ..RouteOptions::new(ModelRouteReason::Retry, ModelThinkingLevel::Low)
                },
                &catalog,
            )
            .await
            .unwrap();
    }

    let g = seen.lock().unwrap();
    assert_eq!(
        g.reasons,
        [ModelRouteReason::Retry, ModelRouteReason::Retry]
    );
    let reported = g.failed[0].as_ref().expect("a physical failed request");
    assert_eq!(reported.model.id.as_str(), "large");
    assert_eq!(
        reported.message.error_message.as_deref(),
        Some("overloaded_error")
    );
    assert_eq!(reported.message.stop_reason, StopReason::Error);
    assert!(
        g.failed[1].is_none(),
        "a failed ROUTING attempt names the virtual model: there is nothing physical to report"
    );
    // previous is the latest successful response, not the failed request.
    assert_eq!(
        g.previous[0]
            .as_ref()
            .map(|p| p.model.id.as_str().to_string()),
        Some("small".to_string())
    );
}

/// pi `if (!target) throw new Error(`${routed}, which is not a physical model.`)` (`:1021`), with
/// the exact text, for BOTH an unknown model and another VIRTUAL model — upstream drives both
/// through the same message (`test/virtual-models.test.ts:198-207`).
/// RED-PROVEN by validating with `get_model` instead of `physical_model`: the virtual target is
/// accepted and `resolve_model` hands a `pi-virtual` model to the caller, which no provider can
/// stream.
#[tokio::test]
async fn resolve_model_refuses_a_route_to_a_virtual_or_unknown_model() {
    let unknown = chat("faux", "missing", 1, 1, false);
    let other_virtual = create_virtual_model(&spec("router", "other"));
    let catalog = Catalog::faux().with(other_virtual.clone());
    for (target, expected) in [
        (
            unknown,
            "Virtual model router/auto routed to faux/missing, which is not a physical model.",
        ),
        (
            other_virtual,
            "Virtual model router/auto routed to router/other, which is not a physical model.",
        ),
    ] {
        let registry = registry_with(Recorder::answering(target));
        let err = registry
            .resolve_model(
                &router_auto(),
                &[],
                route_opts(ModelThinkingLevel::Low),
                &catalog,
            )
            .await
            .expect_err("the route is refused");
        assert_eq!(err.to_string(), expected);
    }
}

/// pi `if (!this.hasConfiguredAuth(target.provider)) throw new Error(`${routed}, which has no
/// credentials.`)` (`:1022`), with the exact text. RED-PROVEN by deleting the auth check: the route
/// succeeds and the request reaches a provider that cannot authenticate it.
#[tokio::test]
async fn resolve_model_refuses_a_route_to_a_provider_without_credentials() {
    let registry = registry_with(Recorder::answering(large()));
    let err = registry
        .resolve_model(
            &router_auto(),
            &[],
            route_opts(ModelThinkingLevel::High),
            &Catalog::faux().unconfigured(),
        )
        .await
        .expect_err("faux has no credentials");
    assert_eq!(
        err.to_string(),
        "Virtual model router/auto routed to faux/large, which has no credentials."
    );
}

/// The router's own failure reaches the caller unchanged, because that text becomes the run's error
/// response (pi: *"If `route()` throws … the request ends with an error response"*).
/// RED-PROVEN by replacing the propagated message with a generic one: the assertion on the router's
/// own words fails.
#[tokio::test]
async fn resolve_model_surfaces_the_routers_own_failure() {
    let registry = registry_with(Arc::new(Failing("no model fits this prompt")));
    let err = registry
        .resolve_model(
            &router_auto(),
            &[],
            route_opts(ModelThinkingLevel::Low),
            &Catalog::faux(),
        )
        .await
        .expect_err("the router failed");
    assert_eq!(err.to_string(), "no model fits this prompt");
    assert!(matches!(err, VirtualModelError::Route(_)));
}

/// `state` goes in from the caller and comes back out untouched (pi `:1012`, `:1024`): the registry
/// neither reads nor writes the session, so the branch-state round trip is entirely the caller's.
/// RED-PROVEN two ways: dropping `state: route.state` from the returned `ModelRoute` makes the
/// returned state `None`; dropping `state: options.state` from the request makes the handed-in
/// state `None`.
#[tokio::test]
async fn resolve_model_passes_router_state_through_in_both_directions() {
    let registry = registry_with(Recorder::returning_state(json!({ "turns": 2 })));
    let stored = json!({ "turns": 1 });
    let route = registry
        .resolve_model(
            &router_auto(),
            &[],
            RouteOptions {
                state: Some(&stored),
                ..route_opts(ModelThinkingLevel::High)
            },
            &Catalog::faux(),
        )
        .await
        .unwrap();
    assert_eq!(route.state, Some(json!({ "turns": 2 })));

    // And the stored state is what the router was handed.
    let (recorder, seen) = Recorder::new();
    let registry = registry_with(recorder);
    registry
        .resolve_model(
            &router_auto(),
            &[],
            RouteOptions {
                state: Some(&stored),
                ..route_opts(ModelThinkingLevel::High)
            },
            &Catalog::faux(),
        )
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().states[0], Some(json!({ "turns": 1 })));
}

/// The reason reaches the router verbatim, in pi's four spellings — a router matching on it is the
/// normal case (upstream's own example routes `direct` to a fixed cheap model).
/// REGRESSION GUARD for the spellings (`as_str` is data, not behaviour); the PASS-THROUGH half is
/// RED-PROVEN by hardcoding `reason: ModelRouteReason::User` in `resolve_model`, which fails the
/// recorded sequence.
#[tokio::test]
async fn resolve_model_hands_the_reason_to_the_router() {
    let (router, seen) = Recorder::new();
    let registry = registry_with(router);
    let catalog = Catalog::faux();
    for reason in [
        ModelRouteReason::User,
        ModelRouteReason::Continuation,
        ModelRouteReason::Retry,
        ModelRouteReason::Direct,
    ] {
        registry
            .resolve_model(
                &router_auto(),
                &[],
                RouteOptions::new(reason, ModelThinkingLevel::High),
                &catalog,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        seen.lock().unwrap().reasons,
        [
            ModelRouteReason::User,
            ModelRouteReason::Continuation,
            ModelRouteReason::Retry,
            ModelRouteReason::Direct
        ]
    );
    assert_eq!(
        ["user", "continuation", "retry", "direct"],
        [
            ModelRouteReason::User.as_str(),
            ModelRouteReason::Continuation.as_str(),
            ModelRouteReason::Retry.as_str(),
            ModelRouteReason::Direct.as_str()
        ]
    );
}

/// Upstream's own second-registration case: a virtual model under a PHYSICAL provider routes just
/// like one under a bare id — `resolveModel(fast, …)` resolves to `faux/small`
/// (`test/virtual-models.test.ts:184-188`). RED-PROVEN by keying the router lookup on the model id
/// alone: `faux/auto` and `router/auto` collide and the wrong router answers.
#[tokio::test]
async fn resolve_model_keys_the_router_on_the_provider_and_the_id() {
    let registry = VirtualModelRegistry::new();
    registry
        .register(
            definition("router", "auto", Recorder::answering(large())),
            &Catalog::faux(),
        )
        .unwrap();
    registry
        .register(
            definition("faux", "auto", Recorder::answering(small())),
            &Catalog::faux(),
        )
        .unwrap();
    let catalog = Catalog::faux();
    let under_faux = create_virtual_model(&spec("faux", "auto"));

    let a = registry
        .resolve_model(
            &router_auto(),
            &[],
            route_opts(ModelThinkingLevel::High),
            &catalog,
        )
        .await
        .unwrap();
    let b = registry
        .resolve_model(
            &under_faux,
            &[],
            route_opts(ModelThinkingLevel::High),
            &catalog,
        )
        .await
        .unwrap();
    assert_eq!(a.model.id.as_str(), "large");
    assert_eq!(b.model.id.as_str(), "small");
}

/// `branch_selection`'s closure tests its answer through `is_virtual_api`, which reads
/// `ModelRef::api`, so `model_ref` MUST populate the api — otherwise a resumed session reads a
/// virtual selection as physical and restores the model that answered last.
/// RED-PROVEN by answering `api: None`: the `pi-virtual` assertion fails.
#[test]
fn model_ref_carries_the_virtual_api_so_a_selection_reads_as_virtual() {
    let registry = VirtualModelRegistry::new();
    let (router, _) = Recorder::new();
    registry
        .register(definition("router", "auto", router), &Catalog::faux())
        .unwrap();
    let selection = registry.model_ref("router", "auto").expect("registered");
    assert_eq!(selection.provider.as_str(), "router");
    assert_eq!(selection.model.as_str(), "auto");
    assert_eq!(
        selection.api.as_ref().map(|a| a.as_str()),
        Some(VIRTUAL_MODEL_API)
    );
    assert!(registry.model_ref("router", "missing").is_none());
}

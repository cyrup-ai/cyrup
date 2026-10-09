//! Tests for the bundled worked router ([`super`]).
//!
//! Everything here drives the PURE [`super::choose`] / [`super::targets`] /
//! [`super::gate_enabled_from`] so no test mutates process env — this crate is
//! `#![forbid(unsafe_code)]` and cannot reach `std::env::set_var`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_core::{CancelToken, ModelThinkingLevel, ProviderId, StopReason};
use cyrup_provider::{
    FailedRequest, Model, ModelRouteReason, ModelRouteRequest, ModelRouter, RoutedModel,
};

use super::{ROUTER_MODEL_ID, ROUTER_PROVIDER, choose, definition, gate_enabled_from, targets};

fn physical(id: &str, context_window: u64) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: ProviderId::from("faux"),
        base_url: "http://localhost:0".to_string(),
        reasoning: true,
        input: vec![cyrup_provider::Modality::Text],
        cost: cyrup_provider::ModelCost::default(),
        context_window,
        max_tokens: 4096,
        sampling_params: None,
        input_limits: None,
        prompt_cache: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn catalog() -> Vec<Model> {
    vec![physical("small", 1_000), physical("large", 50_000)]
}

/// The catalog a request carries — pi's per-request `ctx.modelRegistry`
/// ([`ModelRouteRequest::catalog`]). `has_configured_auth` answers `true` for everything because
/// `choose` never asks it; the credential check belongs to `resolve_model`, not to a router.
struct TestCatalog(Vec<Model>);

impl cyrup_provider::VirtualModelCatalog for TestCatalog {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.0
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
            .cloned()
    }
    fn models(&self) -> Vec<Model> {
        self.0.clone()
    }
    fn has_configured_auth(&self, _provider: &str) -> bool {
        true
    }
    fn has_physical_provider(&self, provider: &str) -> bool {
        self.0.iter().any(|m| m.provider.as_str() == provider)
    }
}

/// A request for `router/auto`, with every optional field absent unless the caller sets it.
struct Req {
    model: Model,
    thinking_level: ModelThinkingLevel,
    reason: ModelRouteReason,
    previous: Option<RoutedModel>,
    failed: Option<FailedRequest>,
    state: Option<serde_json::Value>,
}

impl Req {
    fn new(reason: ModelRouteReason, thinking_level: ModelThinkingLevel) -> Self {
        Self {
            model: cyrup_provider::create_virtual_model(&definition_spec()),
            thinking_level,
            reason,
            previous: None,
            failed: None,
            state: None,
        }
    }

    fn route(&self, small: Option<&str>, large: Option<&str>) -> cyrup_provider::ModelRoute {
        let request = ModelRouteRequest {
            model: &self.model,
            thinking_level: self.thinking_level,
            reason: self.reason,
            previous: self.previous.clone(),
            failed: self.failed.clone(),
            state: self.state.as_ref(),
            messages: &[],
            cancel: CancelToken::new(),
            catalog: &TestCatalog(catalog()),
        };
        choose(small, large, &request).expect("routes")
    }
}

fn definition_spec() -> cyrup_provider::VirtualModelSpec {
    cyrup_provider::VirtualModelSpec {
        provider: ProviderId::from(ROUTER_PROVIDER),
        id: ROUTER_MODEL_ID.into(),
        name: "Auto".to_string(),
        thinking_levels: Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]),
        context_window: None,
        max_tokens: None,
        input: None,
    }
}

/// The gate is OFF by default and ON for anything that is not `0`/`false`/empty.
///
/// RED-PROVE: make `gate_enabled_from` answer `value.is_some()` — the `Some("0")` and
/// `Some("false")` cases fail; make it answer `true` unconditionally — the `None` case fails, which
/// is the one that keeps every existing session's catalog unchanged.
#[test]
fn the_bundled_router_is_absent_unless_opted_in() {
    assert!(!gate_enabled_from(None));
    assert!(!gate_enabled_from(Some("")));
    assert!(!gate_enabled_from(Some("0")));
    assert!(!gate_enabled_from(Some("false")));
    assert!(!gate_enabled_from(Some("FALSE")));
    assert!(gate_enabled_from(Some("1")));
    assert!(gate_enabled_from(Some("true")));
    assert!(gate_enabled_from(Some("yes")));
}

/// The registration names `router/auto` and offers exactly the docs page's two levels, with the
/// limits left UNSET so they fall back to the physical model that answers.
///
/// RED-PROVE: set `context_window: Some(1_000)` in `definition` — the `context_window == 0`
/// assertion fails (and a routed session would then report the router's own declared window until
/// `limits_model` found a response); change `thinking_levels` to `None` — the offered-levels
/// assertion comes back `[Off]`.
#[test]
fn the_bundled_router_registers_router_auto() {
    struct Never;
    #[async_trait::async_trait]
    impl ModelRouter for Never {
        async fn route(
            &self,
            _r: ModelRouteRequest<'_>,
        ) -> Result<cyrup_provider::ModelRoute, cyrup_provider::ModelRouteError> {
            unreachable!("not called")
        }
    }
    let def = definition(Arc::new(Never));
    assert_eq!(def.spec.provider.as_str(), "router");
    assert_eq!(def.spec.id.as_str(), "auto");
    assert_eq!(
        def.spec.thinking_levels,
        Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High])
    );
    let model = cyrup_provider::create_virtual_model(&def.spec);
    assert!(cyrup_provider::is_virtual_model(&model));
    assert_eq!(model.context_window, 0, "unset limits are unknown");
    assert_eq!(model.max_tokens, 0);
    assert_eq!(
        cyrup_provider::get_supported_thinking_levels(&model),
        vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]
    );
}

/// The selected level picks the target on a USER turn: `high` → the large model, anything else →
/// the small one.
///
/// RED-PROVE: drop the `== High` test and always answer `small` — the `high` case fails.
#[test]
fn the_bundled_router_picks_by_the_selected_thinking_level() {
    let high = Req::new(ModelRouteReason::User, ModelThinkingLevel::High).route(None, None);
    assert_eq!(high.model.id.as_str(), "large");
    assert_eq!(high.thinking_level, ModelThinkingLevel::High);

    let low = Req::new(ModelRouteReason::User, ModelThinkingLevel::Low).route(None, None);
    assert_eq!(low.model.id.as_str(), "small");
}

/// A CONTINUATION stays on the model the turn was answered with, at that turn's level — the docs
/// page's sticky rule. A USER turn ignores `previous` entirely.
///
/// RED-PROVE: drop the `reason != User` guard — the user-turn case re-uses `previous` and comes
/// back `large` where the selected level says `small`; drop the sticky arm altogether — the
/// continuation comes back `small` at `Low`, losing both the model and the level, which is exactly
/// the prompt-cache and thinking-signature loss the rule exists to prevent.
#[test]
fn the_bundled_router_keeps_a_continuation_on_the_previous_model() {
    let mut req = Req::new(ModelRouteReason::Continuation, ModelThinkingLevel::Low);
    req.previous = Some(RoutedModel {
        model: physical("large", 50_000),
        thinking_level: Some(ModelThinkingLevel::High),
    });
    let routed = req.route(None, None);
    assert_eq!(routed.model.id.as_str(), "large");
    assert_eq!(routed.thinking_level, ModelThinkingLevel::High);

    // The same `previous`, but a USER turn: the selected level decides.
    let mut user = Req::new(ModelRouteReason::User, ModelThinkingLevel::Low);
    user.previous = req.previous.clone();
    assert_eq!(user.route(None, None).model.id.as_str(), "small");
}

/// A RETRY prefers `failed` over `previous`, and a sticky entry with no recorded level falls back
/// to `medium`.
///
/// RED-PROVE: order the sticky lookup `previous ?? failed` — the retry goes back to `small` (the
/// turn before) instead of to `large` (the request that failed); replace the
/// `unwrap_or(Medium)` with `unwrap_or(Off)` — the level assertion fails.
#[test]
fn the_bundled_router_retries_on_the_model_whose_request_failed() {
    let mut req = Req::new(ModelRouteReason::Retry, ModelThinkingLevel::Low);
    req.previous = Some(RoutedModel {
        model: physical("small", 1_000),
        thinking_level: Some(ModelThinkingLevel::Low),
    });
    req.failed = Some(FailedRequest {
        model: physical("large", 50_000),
        // No recorded level: the `?? "medium"` arm of the docs page's example.
        thinking_level: None,
        message: cyrup_core::AssistantMessage::errored(
            ProviderId::from("faux"),
            "large",
            None,
            StopReason::Error,
            "overloaded_error",
        ),
    });
    let routed = req.route(None, None);
    assert_eq!(routed.model.id.as_str(), "large");
    assert_eq!(routed.thinking_level, ModelThinkingLevel::Medium);
}

/// A DIRECT request goes to the cheap model and returns NO state — "`direct` requests have no
/// state, and Pi ignores state they return".
///
/// RED-PROVE: route `direct` through the sticky arm (drop the early return) — with a `previous`
/// set it comes back `large`, and it starts carrying state the session discards.
#[test]
fn the_bundled_router_sends_direct_requests_to_the_cheap_model_without_state() {
    let mut req = Req::new(ModelRouteReason::Direct, ModelThinkingLevel::High);
    req.previous = Some(RoutedModel {
        model: physical("large", 50_000),
        thinking_level: Some(ModelThinkingLevel::High),
    });
    let routed = req.route(None, None);
    assert_eq!(routed.model.id.as_str(), "small");
    assert_eq!(routed.state, None);
}

/// The turn counter round-trips: absent state starts at 1, and a handed-in state is bumped.
///
/// RED-PROVE: return `state: None` from the non-direct arms — both assertions fail and nothing is
/// ever written to the branch, so the `pi.virtual-model-state` round trip is never exercised.
#[test]
fn the_bundled_router_keeps_a_turn_counter_in_router_state() {
    let first = Req::new(ModelRouteReason::User, ModelThinkingLevel::Low).route(None, None);
    assert_eq!(first.state, Some(serde_json::json!({ "turns": 1 })));

    let mut req = Req::new(ModelRouteReason::User, ModelThinkingLevel::Low);
    req.state = Some(serde_json::json!({ "turns": 7 }));
    assert_eq!(
        req.route(None, None).state,
        Some(serde_json::json!({ "turns": 8 }))
    );
}

/// `provider/id` overrides win over the context-window heuristic, and an override that names
/// nothing in the catalog falls back to it rather than failing the request.
///
/// RED-PROVE: ignore the overrides in `targets` — the first assertion comes back `small`.
#[test]
fn the_bundled_router_honours_explicit_targets() {
    let high = Req::new(ModelRouteReason::User, ModelThinkingLevel::High)
        .route(Some("faux/large"), Some("faux/small"));
    assert_eq!(
        high.model.id.as_str(),
        "small",
        "large override is faux/small"
    );

    let unresolvable = Req::new(ModelRouteReason::User, ModelThinkingLevel::High)
        .route(None, Some("nope/nothing"));
    assert_eq!(unresolvable.model.id.as_str(), "large");
}

/// A VIRTUAL row in the catalog is never a target — "A virtual model cannot route to another
/// virtual model" — and a catalog with nothing else is an error rather than a route the registry
/// would refuse.
///
/// RED-PROVE: drop the `is_virtual_model` filter from `targets` — the virtual row's
/// `context_window == 0` makes it the `min_by_key` winner, so every low-level request routes to
/// `router/auto` itself and `resolve_model` then fails the run with
/// "routed to router/auto, which is not a physical model".
#[test]
fn the_bundled_router_never_routes_to_a_virtual_model() {
    let virtual_row = cyrup_provider::create_virtual_model(&definition_spec());
    let mut with_virtual = catalog();
    with_virtual.push(virtual_row.clone());
    let t = targets(&with_virtual, None, None).expect("has physical models");
    assert_eq!(t.small.id.as_str(), "small");
    assert_eq!(t.large.id.as_str(), "large");

    let err = targets(&[virtual_row], None, None).expect_err("no physical models");
    assert!(
        err.to_string().contains("no physical models to route to"),
        "{err}"
    );
}

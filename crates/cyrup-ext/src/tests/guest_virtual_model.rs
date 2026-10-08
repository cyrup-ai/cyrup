//! The GUEST-tier virtual-model seam, host side, in process: the two `registration` imports, the
//! `host-router.is-route-cancelled` poll and its binding teardown, and [`GuestModelRouter`]'s
//! request shaping and answer decoding.
//!
//! The live round trip through a real component is in `crates/cyrup-it/tests/ext/`; everything here
//! is reachable without a store, because every import impl is a method on `HostState` and
//! `GuestModelRouter` is an ordinary [`cyrup_provider::ModelRouter`]. That split is the same one
//! `registration_validation.rs` makes, and for the same reason: the shapes a wrong JSON key or a
//! leaked token break are cheap to pin and expensive to notice.
//!
//! pi: `pi.registerVirtualModel(model)` / `pi.unregisterVirtualModel(provider, id)`
//! (`core/extensions/types.ts:1865-1876` @v1.0.4), `ModelRouteRequest` / `ModelRoute`
//! (`core/virtual-models.ts:52-82`), `ModelRouteRequest.signal` (`:69`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use crate::host::live::bindings::cyrup::ext::host_router::Host as HostRouterHost;
use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;
use crate::host::{GuestModelRouter, GuestState, HostState, StoreLimits};
use crate::registry::ExtensionRegistry;
use cyrup_core::{CancelToken, ExtensionId, ModelThinkingLevel, ProviderId};
use cyrup_provider::{
    FailedRequest, Modality, Model, ModelCost, ModelRouteReason, ModelRouteRequest, ModelRouter,
    RoutedModel, VirtualModelSpec,
};
use serde_json::{Value, json};

fn guest_host(id: &str) -> (Arc<ExtensionRegistry>, Arc<GuestState>, HostState) {
    let registry = Arc::new(ExtensionRegistry::new());
    let guest = Arc::new(GuestState::new(ExtensionId::from(id), registry.clone()));
    let state = HostState::with_guest(StoreLimits::default(), guest.clone());
    (registry, guest, state)
}

/// pi's `Omit<VirtualModelDefinition, "route">` as a guest writes it, in pi's camelCase.
fn spec_json(provider: &str, id: &str) -> String {
    json!({
        "provider": provider,
        "id": id,
        "name": "Demo Auto Router",
        "thinkingLevels": ["low", "high"],
        "contextWindow": 200_000u64,
        "maxTokens": 8_192u64
    })
    .to_string()
}

fn physical(provider: &str, id: &str, context_window: u64) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: ProviderId::from(provider),
        base_url: "https://example.invalid".into(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window,
        max_tokens: 4_096,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

// ---------------------------------------------------------------------------
// The two registration imports.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_guest_registration_lands_in_the_same_registry_the_native_tier_feeds() {
    let (registry, _guest, mut state) = guest_host("demo");

    state
        .register_virtual_model(spec_json("router", "demo-auto"))
        .await
        .expect("a well-formed spec registers");

    assert_eq!(
        registry.virtual_model_keys().unwrap(),
        vec![("router".to_string(), "demo-auto".to_string())],
        "the guest tier must reuse `ExtensionRegistry::register_virtual_model` — the door the \
         native tier already uses — so the pre-bind queue, the bind-time flush, the owner \
         attribution and the `/reload` purge are inherited rather than forked"
    );
    assert_eq!(
        registry.virtual_model_pending_pairs().unwrap(),
        vec![("router".to_string(), "demo-auto".to_string())],
        "registered before any sink is bound, it is QUEUED — pi's \
         `pendingVirtualModelRegistrations` (`extensions/loader.ts:190`)"
    );
}

/// The guest wire format is pi's OWN key names, which is the whole reason
/// [`cyrup_provider::VirtualModelSpec`] carries `#[serde(rename_all = "camelCase")]`: a guest author
/// writes the object a pi extension author writes (`Omit<VirtualModelDefinition, "route">`,
/// `core/virtual-models.ts:84-102` @v1.0.4).
///
/// Pinned separately from the registration test above because every optional member is
/// `#[serde(default)]` — drop the rename and `thinkingLevels` / `contextWindow` / `maxTokens`
/// deserialize to `None` SILENTLY, the registration still succeeds, and a virtual model reaches the
/// catalog offering `["off"]` and an unknown context window. Registration assertions cannot see
/// that; this can.
#[test]
fn the_guest_spec_wire_format_is_pis_camel_case_in_both_directions() {
    let spec: VirtualModelSpec =
        serde_json::from_str(&spec_json("router", "demo-auto")).expect("the guest spec decodes");

    assert_eq!(
        spec.thinking_levels.as_deref(),
        Some([ModelThinkingLevel::Low, ModelThinkingLevel::High].as_slice()),
        "`thinkingLevels` must reach `thinking_levels`; a snake_case-only struct drops it to \
         `None` and `create_virtual_model` then offers pi's `[\"off\"]` default instead"
    );
    assert_eq!(
        spec.context_window,
        Some(200_000),
        "`contextWindow` must reach `context_window` — dropped, the model's window reads 0"
    );
    assert_eq!(
        spec.max_tokens,
        Some(8_192),
        "`maxTokens` must reach `max_tokens` — dropped, the model's cap reads 0"
    );

    // The SERIALIZE half matters too: the host hands a spec back over the same seam (and into the
    // `pi.virtual-model-state` neighbourhood), so the keys it writes must be the ones a guest reads.
    let back: Value = serde_json::to_value(&spec).expect("a spec serializes");
    for key in ["thinkingLevels", "contextWindow", "maxTokens"] {
        assert!(
            back.get(key).is_some(),
            "serialization must emit pi's `{key}`, not a snake_case sibling: {back}"
        );
    }
}

#[tokio::test]
async fn a_spec_this_host_cannot_read_is_the_imports_err_arm_and_registers_nothing() {
    let (registry, _guest, mut state) = guest_host("demo");

    let err = state
        .register_virtual_model("{\"provider\":\"router\"}".to_string())
        .await
        .expect_err(
            "a spec missing `id`/`name` is a guest author's typo, which pi surfaces as its \
             throwing factory; EXT-082's precedent is to hand it back as the import's `err`",
        );
    assert!(
        err.contains("not a virtual model spec"),
        "the message must name what went wrong: {err}"
    );
    assert!(
        registry.virtual_model_keys().unwrap().is_empty(),
        "a refused registration registers NOTHING"
    );
}

#[tokio::test]
async fn unregister_is_per_pair_and_narrowed_to_the_owner() {
    let (registry, _guest, mut state) = guest_host("demo");
    state
        .register_virtual_model(spec_json("router", "demo-auto"))
        .await
        .unwrap();
    state
        .register_virtual_model(spec_json("router", "second"))
        .await
        .unwrap();

    state
        .unregister_virtual_model("router".into(), "demo-auto".into())
        .await;

    assert_eq!(
        registry.virtual_model_keys().unwrap(),
        vec![("router".to_string(), "second".to_string())],
        "pi's `unregisterVirtualModel(provider, id)` removes ONE pair \
         (`model-runtime.ts:976-982`); a sibling under the same provider id survives"
    );

    // Owner narrowing: another extension's registration is not this guest's to remove.
    let other = Arc::new(GuestState::new(
        ExtensionId::from("intruder"),
        registry.clone(),
    ));
    let mut other_state = HostState::with_guest(StoreLimits::default(), other);
    other_state
        .unregister_virtual_model("router".into(), "second".into())
        .await;
    assert_eq!(
        registry.virtual_model_keys().unwrap(),
        vec![("router".to_string(), "second".to_string())],
        "the ownership narrowing is what stops one guest removing another's virtual model"
    );
}

#[tokio::test]
async fn unregistering_a_provider_does_not_remove_its_virtual_models() {
    let (registry, _guest, mut state) = guest_host("demo");
    state
        .register_virtual_model(spec_json("router", "demo-auto"))
        .await
        .unwrap();

    state.unregister_provider("router".into()).await;

    assert_eq!(
        registry.virtual_model_keys().unwrap(),
        vec![("router".to_string(), "demo-auto".to_string())],
        "the docs page is explicit: \"`pi.unregisterVirtualModel(provider, id)` removes it; \
         `pi.unregisterProvider()` does not.\""
    );
}

// ---------------------------------------------------------------------------
// pi's `request.signal`, as the `route-id`-keyed poll.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cancellation_poll_answers_only_for_the_live_route_id() {
    let (_registry, guest, mut state) = guest_host("demo");
    let cancel = CancelToken::new();
    guest.set_route_cancel(Some(("route-7".into(), cancel.clone())));

    assert!(!state.is_route_cancelled("route-7".into()).await);
    cancel.cancel();
    assert!(
        state.is_route_cancelled("route-7".into()).await,
        "pi's `signal.aborted` must turn true for the route that is running"
    );
    assert!(
        !state.is_route_cancelled("route-8".into()).await,
        "a stale poll from a later handler — or a forged id — must never read another route's \
         cancellation (EXT-M06)"
    );
}

#[tokio::test]
async fn the_binding_is_torn_down_when_the_route_future_is_dropped_mid_await() {
    let (_registry, guest, mut state) = guest_host("demo");
    let cancel = CancelToken::new();
    cancel.cancel();

    // The third exit path a Rust future has, which `ToolCallBinding`'s doc records and a JS port
    // cannot think of: the whole dispatch future dropped at its await point by an outer
    // `timeout`/`select!`. `route_model` makes it reachable BY CONSTRUCTION, because it wraps
    // lock-acquisition-plus-call in a `tokio::time::timeout`.
    {
        guest.set_route_cancel(Some(("route-9".into(), cancel.clone())));
        let _binding = crate::host::live::RouteCallBinding(&guest);
        assert!(state.is_route_cancelled("route-9".into()).await);
    }

    assert!(
        !state.is_route_cancelled("route-9".into()).await,
        "the abandoned route's token must be UNBOUND by the guard's `Drop`, not by a hand-written \
         clear on each `select!` arm — which does not run on the dropped-future path. A leaked \
         token answers `true` to every later poll, from any handler, until the next route happens \
         to rebind it."
    );
}

// ---------------------------------------------------------------------------
// GuestModelRouter: request shaping, answer decoding, and the no-longer-loaded answer.
// ---------------------------------------------------------------------------

fn router(live: Option<std::sync::Weak<crate::facade::LiveMap>>) -> GuestModelRouter {
    GuestModelRouter {
        live,
        owner: ExtensionId::from("demo"),
        provider: "router".into(),
        id: "demo-auto".into(),
    }
}

fn virtual_model() -> Model {
    cyrup_provider::create_virtual_model(&VirtualModelSpec {
        provider: ProviderId::from("router"),
        id: "demo-auto".into(),
        name: "Demo".into(),
        thinking_levels: Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]),
        context_window: None,
        max_tokens: None,
        input: None,
    })
}

#[tokio::test]
async fn a_router_whose_extension_is_no_longer_loaded_fails_the_route_by_name() {
    let model = virtual_model();
    let request = ModelRouteRequest {
        model: &model,
        thinking_level: ModelThinkingLevel::High,
        reason: ModelRouteReason::User,
        previous: None,
        failed: None,
        state: None,
        messages: &[],
        cancel: CancelToken::new(),
        catalog: &cyrup_provider::NoCatalog,
    };

    // A dead `Weak`: the live-instance table the facade owned is gone, which is the state after a
    // `/reload` drops the owner. Upgrading must FAIL rather than panic, and the route must say so.
    let dropped: Arc<crate::facade::LiveMap> =
        Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
    let weak = Arc::downgrade(&dropped);
    drop(dropped);

    let err = router(Some(weak))
        .route(request)
        .await
        .expect_err("a route against an unloaded extension cannot succeed");
    assert!(
        err.0.contains("router/demo-auto") && err.0.contains("no longer loaded"),
        "the message must name the pair and the reason, because it reaches the user as the \
         request's error response: {}",
        err.0
    );
}

#[tokio::test]
async fn a_router_built_without_a_live_table_fails_rather_than_pretending_to_route() {
    let model = virtual_model();
    let request = ModelRouteRequest {
        model: &model,
        thinking_level: ModelThinkingLevel::Low,
        reason: ModelRouteReason::Continuation,
        previous: None,
        failed: None,
        state: None,
        messages: &[],
        cancel: CancelToken::new(),
        catalog: &cyrup_provider::NoCatalog,
    };
    assert!(
        router(None).route(request).await.is_err(),
        "a `GuestState` built outside an `ExtensionHost` has no instance to route into; answering \
         a route anyway would stream a fabricated model"
    );
}

/// The request shaping, read back as the JSON a guest receives. This is the test that catches a
/// wire shape which is merely plausible: every one of these keys is pi's, and a renamed or
/// `null`-instead-of-absent member silently disables the router arm that reads it.
#[test]
fn the_request_json_is_pis_model_route_request_in_camel_case() {
    let model = virtual_model();
    let previous_model = physical("faux", "small", 50_000);
    let failed_model = physical("faux", "large", 100_000);
    let failure = cyrup_core::AssistantMessage::errored(
        ProviderId::from("faux"),
        "large",
        None,
        cyrup_core::StopReason::Error,
        "overloaded_error",
    );
    let state = json!({ "turns": 2 });

    let request = ModelRouteRequest {
        model: &model,
        thinking_level: ModelThinkingLevel::High,
        reason: ModelRouteReason::Retry,
        previous: Some(RoutedModel {
            model: previous_model,
            thinking_level: Some(ModelThinkingLevel::Off),
        }),
        failed: Some(FailedRequest {
            model: failed_model,
            thinking_level: Some(ModelThinkingLevel::High),
            message: failure,
        }),
        state: Some(&state),
        messages: &[],
        cancel: CancelToken::new(),
        catalog: &cyrup_provider::NoCatalog,
    };

    let wire = GuestModelRouter::request_json(&request);
    assert_eq!(
        wire["model"]["id"],
        json!("demo-auto"),
        "pi's `model` is the selected VIRTUAL model, not the one being routed to"
    );
    assert_eq!(wire["thinkingLevel"], json!("high"));
    assert_eq!(
        wire["reason"],
        json!("retry"),
        "the four reason strings are what a router matches on and what upstream's tests assert"
    );
    assert_eq!(wire["previous"]["model"]["id"], json!("small"));
    assert_eq!(wire["previous"]["thinkingLevel"], json!("off"));
    assert_eq!(wire["failed"]["model"]["id"], json!("large"));
    assert_eq!(
        wire["failed"]["message"]["errorMessage"],
        json!("overloaded_error"),
        "pi's own suite pins this value reaching the router, because it is how a router tells an \
         overload (re-route) from a tool-format error (stay put)"
    );
    assert_eq!(wire["state"], state);
    assert!(wire["messages"].is_array());
    assert!(
        wire.get("signal").is_none(),
        "pi's `signal` is NOT on the wire: it is the `route-id`-keyed poll, because a CancelToken \
         is not a WIT value"
    );
}

#[test]
fn an_absent_optional_is_omitted_from_the_request_rather_than_sent_as_null() {
    let model = virtual_model();
    let request = ModelRouteRequest {
        model: &model,
        thinking_level: ModelThinkingLevel::Low,
        reason: ModelRouteReason::User,
        previous: None,
        failed: None,
        state: None,
        messages: &[],
        cancel: CancelToken::new(),
        catalog: &cyrup_provider::NoCatalog,
    };
    let wire = GuestModelRouter::request_json(&request);
    for key in ["previous", "failed", "state"] {
        assert!(
            wire.get(key).is_none(),
            "`{key}` must be ABSENT, as pi's optional members are — a guest reading \
             `\"{key}\" in request` must see what a pi extension sees"
        );
    }
}

#[test]
fn a_whole_model_object_decodes_and_a_provider_id_stub_resolves_out_of_the_catalog() {
    let r = router(None);
    let large = physical("faux", "large", 100_000);
    let catalog = json!([serde_json::to_value(&large).unwrap()]);

    // The answer pi's docs tell a router to return: the object `ctx.modelRegistry.find(…)` gave it,
    // which is this host's own `Model` serialization round-tripped — so it decodes by construction.
    let answer = json!({ "model": large, "thinkingLevel": "high", "state": { "phase": "plan" } })
        .to_string();
    let route = r
        .decode_route(&answer, &catalog)
        .expect("full model decodes");
    assert_eq!(route.model.id.as_str(), "large");
    assert_eq!(route.thinking_level, ModelThinkingLevel::High);
    assert_eq!(route.state, Some(json!({ "phase": "plan" })));

    // `Model` has ten non-defaulted fields, so the `{provider, id}` stub a Rust author will
    // naturally write does NOT deserialize. It is resolved out of the guest's own catalog view
    // instead — safe because the router's model is only an ADDRESS and `resolve_model` returns the
    // CATALOG's row.
    let stub = json!({ "model": { "provider": "faux", "id": "large" }, "thinkingLevel": "low" })
        .to_string();
    let route = r
        .decode_route(&stub, &catalog)
        .expect("a {provider, id} answer resolves out of the catalog");
    assert_eq!(route.model.id.as_str(), "large");
    assert_eq!(
        route.model.context_window, 100_000,
        "the CATALOG's row, not a stub"
    );
    assert_eq!(route.state, None, "an omitted state keeps the current one");
}

#[test]
fn an_answer_naming_nothing_in_the_catalog_is_an_error_not_a_guess() {
    let r = router(None);
    let catalog = json!([]);
    for answer in [
        json!({ "model": { "provider": "faux", "id": "ghost" }, "thinkingLevel": "low" }),
        json!({ "model": "faux/large", "thinkingLevel": "low" }),
        json!({ "thinkingLevel": "low" }),
        json!({ "model": { "provider": "faux", "id": "large" }, "thinkingLevel": "sideways" }),
    ] {
        assert!(
            r.decode_route(&answer.to_string(), &catalog).is_err(),
            "an answer this host cannot turn into a physical model must fail the route — \
             upstream's contract is an error response, never a guessed model: {answer}"
        );
    }
    assert!(
        r.decode_route("not json at all", &catalog).is_err(),
        "a non-JSON answer is a failed route too"
    );
}

#[test]
fn the_guest_route_errors_all_carry_a_message_that_names_the_virtual_model() {
    // Every arm becomes the text of a `ModelRouteError`, which upstream's contract puts in front of
    // the user as the request's error response — so an arm with an unhelpful message is a bug.
    let reentry =
        crate::host::GuestRouteError::Reentry(crate::host::GuestReentry::RouterOfBusyInstance {
            provider: "router".into(),
            id: "demo-auto".into(),
            holder: cyrup_core::ToolCallId::from("call-1"),
        });
    let text = reentry.to_string();
    assert!(
        text.contains("router/demo-auto") && text.contains("call-1"),
        "{text}"
    );

    let timeout = crate::host::GuestRouteError::LockTimeout {
        provider: "router".into(),
        id: "demo-auto".into(),
        timeout_ms: 10_000,
    };
    let text = timeout.to_string();
    assert!(
        text.contains("router/demo-auto") && text.contains("10000"),
        "{text}"
    );
}

/// Non-vacuity for the whole file: the host's `models.list-models` answer is the shape
/// [`a_whole_model_object_decodes_and_a_provider_id_stub_resolves_out_of_the_catalog`] feeds the
/// fallback, so a change to either side that breaks the round trip shows up here rather than as a
/// guest router that mysteriously cannot name a model.
#[test]
fn the_hosts_own_model_serialization_round_trips_through_the_catalog_view() {
    let large = physical("faux", "large", 100_000);
    let wire: Value = serde_json::to_value(&large).unwrap();
    assert_eq!(wire["provider"], json!("faux"));
    assert_eq!(wire["contextWindow"], json!(100_000));
    let back: Model = serde_json::from_value(wire).unwrap();
    assert_eq!(back, large);
}

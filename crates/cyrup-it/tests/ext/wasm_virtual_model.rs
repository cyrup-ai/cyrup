//! The WASM/GUEST extension tier of virtual models, live: a real `wasm32-wasip2` component
//! registers a virtual model from its `init`, and the host routes real requests through the
//! `route()` that runs inside the guest.
//!
//! Upstream this is one call and one callback: `pi.registerVirtualModel({…, route(request, ctx)})`
//! (`core/extensions/types.ts:1865-1872`, `:1886-1889` @v1.0.4; impl
//! `core/extensions/loader.ts:500-513`, which wraps the author's two-argument `route` and creates
//! `runtime.createContext()` PER REQUEST). cyrup could not express it for a guest at all: a router
//! is a CALLABLE and ADR-0002 makes extension I/O values, so only the spec could cross and nothing
//! could be behind it. The round trip that closes it —
//! `registration.register-virtual-model` (declare), `events.route-model` (run) and
//! `host-router.is-route-cancelled` (pi's `request.signal`) — can only be proven with a real
//! component in a real store, which is why these tests are here and not in `cyrup-ext`'s in-process
//! suite.
//!
//! Each test drives the PRODUCTION path end to end: load the guest, bind the model-registry sink
//! the session binds (`ExtensionRegistry::bind_model_registry`), then ask
//! `cyrup_provider::VirtualModelRegistry::resolve_model` for a route exactly as
//! `AgentSession::route_request` does. Nothing here reaches into the guest by a back door.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use crate::fixture;

use cyrup_core::{
    AssistantMessage, CancelToken, ExtensionId, Message, ModelThinkingLevel, ProviderId, StopReason,
};
use cyrup_ext::provider::{ModelRegistrySink, ProviderRegistration};
use cyrup_ext::{CannedResponses, ExtMode, ExtensionHost, HostConfig, RecordingServices};
use cyrup_provider::{
    Modality, Model, ModelCost, ModelRouteReason, RouteOptions, VirtualModelCatalog,
    VirtualModelDefinition, VirtualModelError, VirtualModelRegistry,
};
use serde_json::{Value, json};

/// The pair the demo guest registers (`cyrup-ext-sdk/src/example/virtual_model.rs`).
const PROVIDER: &str = "router";
const ID: &str = "demo-auto";

// ---------------------------------------------------------------------------
// The catalog the demo guest sees, and the one the host routes against.
// ---------------------------------------------------------------------------

fn physical(id: &str, context_window: u64) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: ProviderId::from("faux"),
        base_url: "https://example.invalid".into(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window,
        max_tokens: 4_096,
        sampling_params: None,
        prompt_cache: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// `faux/small` and `faux/large` — upstream's own two-model router fixture
/// (`test/suite/virtual-models.test.ts`), named the same way so the behaviour is comparable.
fn faux_models() -> Vec<Model> {
    vec![physical("small", 50_000), physical("large", 100_000)]
}

/// The host's catalog view for `resolve_model`: the two faux models, both with credentials.
struct Catalog {
    models: Vec<Model>,
    /// Which providers have credentials — `false` is the "model without credentials" arm of
    /// upstream's failure sentence.
    credentials: bool,
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
    fn has_configured_auth(&self, _provider: &str) -> bool {
        self.credentials
    }
    fn has_physical_provider(&self, provider: &str) -> bool {
        self.models.iter().any(|m| m.provider.as_str() == provider)
    }
}

/// The session's sink, narrowed to the one verb these tests need: it feeds every guest registration
/// into a real [`VirtualModelRegistry`], which is what `cyrup-session-svc`'s `VirtualModelSink`
/// does (`crates/cyrup-session-svc/src/guest_providers.rs`).
struct Sink {
    registry: Arc<VirtualModelRegistry>,
    catalog: Mutex<Vec<Model>>,
}

impl ModelRegistrySink for Sink {
    fn upsert_provider(&self, _reg: &ProviderRegistration) {}
    fn upsert_live_provider(&self, _id: &str, _provider: Arc<dyn cyrup_provider::Provider>) {}
    fn remove_provider(&self, _id: &str) {}
    fn upsert_virtual_model(&self, definition: &VirtualModelDefinition) -> Result<(), String> {
        let catalog = Catalog {
            models: self.catalog.lock().unwrap().clone(),
            credentials: true,
        };
        self.registry
            .register(definition.clone(), &catalog)
            .map_err(|e| e.to_string())
    }
    fn remove_virtual_model(&self, provider: &str, id: &str) {
        self.registry.unregister(provider, id);
    }
}

/// Load the demo guest with a `HostServices` whose `models()` answers the faux catalog — that
/// import IS the guest's `ctx.modelRegistry` (`models.list-models`), and the demo router names its
/// target through it, which is what upstream's docs tell a router to do.
async fn host_with_demo() -> (ExtensionHost, Arc<VirtualModelRegistry>) {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = ExtensionHost::with_wasm(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    })
    .expect("host with wasm runtime");
    let services = Arc::new(RecordingServices::new(CannedResponses {
        models: json!(faux_models()),
        ..CannedResponses::default()
    }));
    host.load_wasm("demo".into(), &bytes, services)
        .await
        .expect("load + init");

    let registry = Arc::new(VirtualModelRegistry::new());
    let errors = host
        .registry()
        .bind_model_registry(Arc::new(Sink {
            registry: Arc::clone(&registry),
            catalog: Mutex::new(faux_models()),
        }))
        .expect("bind the model-registry sink");
    assert!(
        errors.is_empty(),
        "the guest's registration must flush cleanly at bind, as pi's \
         `pendingVirtualModelRegistrations` loop does: {errors:?}"
    );
    (host, registry)
}

fn options(reason: ModelRouteReason, level: ModelThinkingLevel) -> RouteOptions<'static> {
    RouteOptions {
        reason,
        thinking_level: level,
        failed: None,
        state: None,
        cancel: CancelToken::new(),
    }
}

fn catalog() -> Catalog {
    Catalog {
        models: faux_models(),
        credentials: true,
    }
}

/// One assistant response naming a physical model, so `find_latest_response` gives the routing step
/// a `previous` — the field the guest's sticky arm reads.
fn answered(model: &str) -> Message {
    let mut a = AssistantMessage::errored(
        ProviderId::from("faux"),
        model,
        None,
        StopReason::Stop,
        String::new(),
    );
    a.stop_reason = StopReason::Stop;
    a.error_message = None;
    a.thinking_level = Some(ModelThinkingLevel::High);
    Message::Assistant(a)
}

// ---------------------------------------------------------------------------
// The non-vacuity anchor: before this, the whole tier was unreachable.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wasm_guest_registers_a_virtual_model_from_its_own_init() {
    let (host, registry) = host_with_demo().await;

    assert_eq!(
        host.registry().virtual_model_keys().unwrap(),
        vec![(PROVIDER.to_string(), ID.to_string())],
        "the guest's `registration.register-virtual-model` call must reach the registry the \
         NATIVE tier already feeds; before this world verb existed a WASM guest could not \
         register a virtual model by any path"
    );
    let entry = registry
        .get(PROVIDER, ID)
        .expect("the bind-time flush must put it in the model registry");
    assert_eq!(
        entry.api.as_str(),
        cyrup_provider::VIRTUAL_MODEL_API,
        "it is a VIRTUAL catalog entry, which is what makes it selectable and routable"
    );
    assert_eq!(
        cyrup_provider::get_supported_thinking_levels(&entry),
        vec![ModelThinkingLevel::Low, ModelThinkingLevel::High],
        "the spec's `thinkingLevels` crossed the seam and shaped the catalog entry's level map"
    );
    drop(host);
}

// ---------------------------------------------------------------------------
// The bar: a guest router routes a real request end to end.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_guests_answer_is_what_the_request_is_routed_to() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    // A fresh user turn at `high`: the demo router's level arm picks the biggest physical model.
    let route = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("the guest router answered and the host accepted the answer");
    assert_eq!(
        (route.model.provider.as_str(), route.model.id.as_str()),
        ("faux", "large"),
        "the ROUTE is the guest's choice, not a default: a forwarder that crosses the boundary and \
         throws the answer away would still produce a route, and this is the assertion that \
         catches it"
    );
    assert_eq!(route.thinking_level, ModelThinkingLevel::High);

    // The same selection at `low` takes the other arm, which proves the request's thinkingLevel
    // reached the guest rather than being re-derived host-side.
    let route = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::Low),
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(route.model.id.as_str(), "small");
    assert_eq!(
        route.thinking_level,
        ModelThinkingLevel::Off,
        "the guest asked for `off`, and the host clamped it against the ANSWERING model"
    );
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn previous_and_failed_reach_the_guest_with_the_values_upstream_pins() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    // A CONTINUATION after a response from `faux/small`: the demo router's sticky arm returns
    // `(failed ?? previous).model`, so the only way the route can name `small` is if `previous`
    // crossed the boundary carrying the physical model the transcript recorded.
    let messages = vec![answered("small")];
    let route = registry
        .resolve_model(
            &model,
            &messages,
            options(ModelRouteReason::Continuation, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(
        route.model.id.as_str(),
        "small",
        "a continuation STAYS on the physical model that answered — upstream's own test router \
         does exactly this, and it is only possible if `previous.model` arrived"
    );
    assert_eq!(
        route.thinking_level,
        ModelThinkingLevel::High,
        "`previous.thinkingLevel` arrived too (PROV-127's field, across the WASM seam)"
    );

    // A RETRY whose failed request was `faux/large`: `failed` wins over `previous`.
    let mut failure = AssistantMessage::errored(
        ProviderId::from("faux"),
        "large",
        None,
        StopReason::Error,
        "overloaded_error",
    );
    failure.thinking_level = Some(ModelThinkingLevel::Low);
    let route = registry
        .resolve_model(
            &model,
            &messages,
            RouteOptions {
                failed: Some(&failure),
                ..options(ModelRouteReason::Retry, ModelThinkingLevel::High)
            },
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(
        route.model.id.as_str(),
        "large",
        "`failed ?? previous` means the FAILED request wins; the transcript still says `small`, \
         so naming `large` can only come from `failed.model` having crossed"
    );
    assert_eq!(route.thinking_level, ModelThinkingLevel::Low);
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn router_state_round_trips_across_the_component_boundary() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    let first = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(
        first.state.as_ref().and_then(|s| s.get("turns")),
        Some(&json!(1)),
        "the guest's returned state must come back as the object it built"
    );

    // Hand that state back as `request.state`, which is what the session does after persisting it.
    let state = first.state.clone().unwrap();
    let second = registry
        .resolve_model(
            &model,
            &[],
            RouteOptions {
                state: Some(&state),
                ..options(ModelRouteReason::User, ModelThinkingLevel::High)
            },
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(
        second.state.as_ref().and_then(|s| s.get("turns")),
        Some(&json!(2)),
        "the counter only advances if the guest SAW the state the host sent; a request that drops \
         `state` leaves the guest at 1 forever"
    );
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_direct_request_routes_without_state_and_at_the_routers_own_level() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    let route = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::Direct, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("a compaction summary routes");
    assert_eq!(route.model.id.as_str(), "large");
    assert_eq!(route.thinking_level, ModelThinkingLevel::Low);
    assert!(
        route.state.is_none(),
        "pi: a `direct` request has no state and any it returns is ignored — the demo router \
         returns none at all, which is the honest guest-side half of that rule"
    );
    drop(host);
}

// ---------------------------------------------------------------------------
// pi's `request.signal`, observed BY THE GUEST.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_reaches_the_guest_through_the_route_id_keyed_poll() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();
    let ext = host
        .live_extension(&ExtensionId::from("demo"))
        .expect("the loaded instance");

    // Cancel AFTER the route has started, which is the case pi's guidance is about: a router doing
    // real work polls `signal` between units of work. Cancelling beforehand proves nothing about
    // the poll — the host's own `select!` would short-circuit first.
    let cancel = CancelToken::new();
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            cancel.cancel();
        });
    }
    // `{"awaitCancel": true}` puts the demo router in its polling loop (see
    // `cyrup-ext-sdk/src/example/virtual_model.rs`).
    let state = json!({ "awaitCancel": true });
    let result = registry
        .resolve_model(
            &model,
            &[],
            RouteOptions {
                state: Some(&state),
                cancel: cancel.clone(),
                ..options(ModelRouteReason::User, ModelThinkingLevel::High)
            },
            &catalog(),
        )
        .await;
    assert!(
        result.is_err(),
        "a cancelled request must not produce a route: {result:?}"
    );

    // The discriminating assertion: the GUEST notified the host the instant its own
    // `is_route_cancelled(route-id)` poll answered true. A host that bound the token to the wrong
    // `route-id` would still fail this request — on its own `select!` arm — and this notification
    // would never appear.
    let notified = ext
        .guest()
        .notifications()
        .into_iter()
        .any(|message| message == cyrup_ext_sdk::example::CANCEL_OBSERVED);
    assert!(
        notified,
        "the guest's `host-router.is-route-cancelled` poll never answered true; the host bound \
         pi's `request.signal` to something the guest could not read. Notifications seen: {:?}",
        ext.guest().notifications()
    );
    drop(host);
}

// ---------------------------------------------------------------------------
// Containment: a guest router cannot hang, crash or deadlock the request.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_route_asked_of_an_instance_inside_a_tool_call_is_refused_not_queued() {
    use cyrup_core::{ToolCallId, ToolUpdate};

    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();
    let ext = host
        .live_extension(&ExtensionId::from("demo"))
        .expect("the loaded instance");

    // Hold the instance with a real guest tool call. A WASM instance runs one call at a time and
    // the holder is SUSPENDED wasm, so queueing a route behind it can wait indefinitely — the
    // hazard CODE-016 names and the one a pi router does not have.
    let holder = {
        let ext = Arc::clone(&ext);
        tokio::spawn(async move {
            let mut sink: cyrup_core::ToolUpdateSink = Box::new(|_: ToolUpdate| {});
            ext.execute_tool(
                "vm_hold",
                &ToolCallId::from("hold-1"),
                &json!({ "ms": 1_500 }),
                &CancelToken::new(),
                &mut sink,
                None,
            )
            .await
        })
    };
    // Wait until the host has actually bound the in-flight call, so the race is not the test's.
    let bound = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while ext.guest().tool_call_in_flight().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(bound.is_ok(), "the guest tool never took the instance");

    // The refusal must come back promptly. The timeout is an ASSERTION, not an abort: a regression
    // here is a hang, and a hung suite reads as infrastructure trouble rather than as a bug.
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        registry.resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        ),
    )
    .await
    .expect("the route must be REFUSED promptly, never queued behind suspended wasm")
    .expect_err("a route onto a busy instance cannot succeed");

    let text = refused.to_string();
    assert!(
        text.contains("executing tool call") && text.contains("hold-1"),
        "the refusal names the holder, because it reaches the user as the request's error \
         response — upstream's contract for a failing router: {text}"
    );
    let _ = holder.await;
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_route_waiting_on_a_busy_instance_is_bounded_rather_than_hanging() {
    let (host, _registry) = host_with_demo().await;
    let ext = host
        .live_extension(&ExtensionId::from("demo"))
        .expect("the loaded instance");

    // Hold the instance from a COMMAND, which takes the same `inner` mutex but binds no in-flight
    // tool call — so the reentrancy refusal above does not apply and the wait itself is what is
    // under test. The epoch budget does NOT cover this wait: every export arms
    // `set_epoch_deadline` only AFTER it holds the lock.
    let holder = {
        let ext = Arc::clone(&ext);
        tokio::spawn(async move {
            ext.execute_command("vm-hold", "1500", &CancelToken::new())
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let started = std::time::Instant::now();
    let err = ext
        .route_model_with_timeout(
            PROVIDER,
            ID,
            "route-bounded",
            "{\"model\":{},\"thinkingLevel\":\"high\",\"reason\":\"user\",\"messages\":[]}",
            &CancelToken::new(),
            std::time::Duration::from_millis(80),
        )
        .await
        .expect_err("the bounded wait must give up rather than block the turn");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1_200),
        "it must give up on ITS OWN bound, not when the holder happens to finish"
    );
    let text = err.to_string();
    assert!(
        text.contains("still busy") && text.contains("router/demo-auto"),
        "the timeout is a typed error naming the virtual model, never a hung user turn: {text}"
    );
    let _ = holder.await;
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_router_that_refuses_ends_the_request_with_its_own_message() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    // An EMPTY catalog view is what the guest's `ctx.models().list()` would answer if the host had
    // no models, so the demo router takes its own refusal arm. That is pi's `route()` THROWING,
    // and upstream's contract is one sentence: the request ends with an error response.
    let bytes = std::fs::read(fixture::component()).unwrap();
    let bare = ExtensionHost::with_wasm(fixture::cfg()).unwrap();
    bare.load_wasm("demo".into(), &bytes, Arc::new(cyrup_ext::DenyServices))
        .await
        .unwrap();
    let bare_registry = Arc::new(VirtualModelRegistry::new());
    bare.registry()
        .bind_model_registry(Arc::new(Sink {
            registry: Arc::clone(&bare_registry),
            catalog: Mutex::new(faux_models()),
        }))
        .unwrap();

    let bare_model = bare_registry.get(PROVIDER, ID).unwrap();
    let err = bare_registry
        .resolve_model(
            &bare_model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect_err("a router that refuses cannot produce a route");
    assert!(
        matches!(err, VirtualModelError::Route(_)),
        "the guest's `err` arm must arrive as the ROUTER's failure, not as some host-side \
         fallback: {err}"
    );
    assert!(
        err.to_string().contains("no physical model to route to"),
        "the guest's own text reaches the user unchanged: {err}"
    );

    // And the host is still alive: the SAME instance answers a later call.
    let _ = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("a contained guest refusal must not poison the host");
    drop((host, bare));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_route_to_a_provider_without_credentials_is_refused_by_name() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    let err = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &Catalog {
                models: faux_models(),
                credentials: false,
            },
        )
        .await
        .expect_err("upstream: \"or returns … a model without credentials\"");
    assert!(
        matches!(err, VirtualModelError::NoCredentials { .. }),
        "the guest answered a real physical model; it is the CREDENTIAL check that refuses: {err}"
    );
    drop(host);
}

// ---------------------------------------------------------------------------
// Reload: the router resolves its instance per call, so the owner leaving is the answer.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_the_owner_is_purged_the_registration_leaves_the_model_registry_too() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();
    // It routes while the owner is loaded, so the assertion after the purge is about the purge.
    registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("routed before the purge");

    host.registry()
        .purge_owner(&ExtensionId::from("demo"))
        .expect("purge the owner, as `/reload` does");

    assert!(
        host.registry().virtual_model_keys().unwrap().is_empty(),
        "cyrup attributes each registration to its owner and drops it with the owner — see \
         `cyrup_ext::virtual_model`'s CYRUP-DELTA on why that is better than upstream's reload \
         leak, where a dead extension's router stays registered"
    );
    assert!(
        registry.get(PROVIDER, ID).is_none(),
        "the purge must reach the MODEL REGISTRY through the sink's `remove_virtual_model`, not \
         only the hub's own table: a catalog entry whose router is gone would be selectable and \
         unroutable"
    );
    let err = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect_err("nothing is registered under the pair any more");
    assert!(
        matches!(err, VirtualModelError::NotRegistered { .. }),
        "and the answer is the typed not-registered refusal, not a panic on an upgraded `Weak`: \
         {err}"
    );
    drop(host);
}

// ---------------------------------------------------------------------------
// Non-vacuity: the catalog view the guest reads is really the host's import.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_guest_names_its_target_through_the_models_import_and_not_a_hard_coded_id() {
    let (host, registry) = host_with_demo().await;
    let model = registry.get(PROVIDER, ID).unwrap();

    // A host whose `models()` answers a DIFFERENT catalog must move the guest's answer with it:
    // the demo router picks by context window out of `ctx.models().list()`, which is
    // `models.list-models`, which is pi's `ctx.modelRegistry`.
    let shifted = vec![physical("small", 400_000), physical("large", 10_000)];
    let bytes = std::fs::read(fixture::component()).unwrap();
    let other = ExtensionHost::with_wasm(fixture::cfg()).unwrap();
    other
        .load_wasm(
            "demo".into(),
            &bytes,
            Arc::new(RecordingServices::new(CannedResponses {
                models: json!(shifted),
                ..CannedResponses::default()
            })),
        )
        .await
        .unwrap();
    let other_registry = Arc::new(VirtualModelRegistry::new());
    other
        .registry()
        .bind_model_registry(Arc::new(Sink {
            registry: Arc::clone(&other_registry),
            catalog: Mutex::new(faux_models()),
        }))
        .unwrap();

    let route = other_registry
        .resolve_model(
            &other_registry.get(PROVIDER, ID).unwrap(),
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(
        route.model.id.as_str(),
        "small",
        "with the windows swapped, the biggest model is now `small` — so the guest read the \
         HOST's live catalog rather than a baked-in id"
    );

    // The original host is unchanged, which rules out shared global state between the two.
    let route = registry
        .resolve_model(
            &model,
            &[],
            options(ModelRouteReason::User, ModelThinkingLevel::High),
            &catalog(),
        )
        .await
        .expect("routed");
    assert_eq!(route.model.id.as_str(), "large");
    let _: Value = json!({});
    drop((host, other));
}

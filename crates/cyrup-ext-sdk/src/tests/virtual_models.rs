//! The guest-side author surface for virtual models (pi `pi.registerVirtualModel`,
//! `core/extensions/types.ts:1865-1876` @v1.0.4), on the host target.
//!
//! Two properties are pinned here because neither needs a seam and both are silent when wrong:
//!
//! 1. **Wire fidelity.** The `route-model` export carries pi's `ModelRouteRequest` and `ModelRoute`
//!    as JSON in camelCase. A renamed or re-cased key is not a compile error on either side — the
//!    host serializes its own names and the guest deserializes its own — so the shapes are checked
//!    against a fixture written out in pi's spelling, exactly as `payload_fidelity.rs` does for the
//!    event payloads.
//! 2. **Dispatch.** [`ExtensionApi::register_virtual_model`] is keyed by `(provider, id)` and a
//!    re-registration REPLACES in place, because upstream's `Map.set` does and the listing order is
//!    observable (`test/virtual-models.test.ts:174-175`). An unregistered pair must come back as an
//!    `Err`, never as a successful route, because a bogus successful answer is the one outcome
//!    upstream's contract rules out.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use crate::prelude::*;

/// pi's `ModelRouteRequest` as the host writes it, in pi's own spelling
/// (`core/virtual-models.ts:52-70` @v1.0.4). `previous`/`failed`/`state` present, because the arms
/// that read them are the ones a wrong key silently disables.
fn request_fixture() -> Value {
    json!({
        "model": { "provider": "router", "id": "demo-auto", "api": "pi-virtual" },
        "thinkingLevel": "high",
        "reason": "retry",
        "previous": {
            "model": { "provider": "faux", "id": "small" },
            "thinkingLevel": "off"
        },
        "failed": {
            "model": { "provider": "faux", "id": "large" },
            "thinkingLevel": "high",
            "message": { "role": "assistant", "stopReason": "error", "errorMessage": "overloaded_error" }
        },
        "state": { "turns": 2, "phase": "sticky" },
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }]
    })
}

#[test]
fn a_route_request_decodes_from_pis_camel_case_field_for_field() {
    let request: ModelRouteRequest = serde_json::from_value(request_fixture()).expect(
        "the host writes pi's camelCase keys; if this fails the guest is reading different names \
         and every optional arm silently goes dead",
    );

    assert_eq!(request.thinking_level, "high");
    assert_eq!(request.reason, "retry");
    let previous = request.previous.expect("`previous` must survive the wire");
    assert_eq!(
        previous.model.get("id").and_then(Value::as_str),
        Some("small")
    );
    assert_eq!(previous.thinking_level.as_deref(), Some("off"));
    let failed = request.failed.expect("`failed` must survive the wire");
    assert_eq!(
        failed.message.get("errorMessage").and_then(Value::as_str),
        Some("overloaded_error"),
        "pi's test pins this exact value reaching the router \
         (`test/suite/virtual-models.test.ts`)"
    );
    assert_eq!(
        request.state.as_ref().and_then(|s| s.get("turns")),
        Some(&json!(2)),
        "the router state the host read off the session branch must arrive as the object it stored"
    );
    assert_eq!(
        request.messages.as_array().map(Vec::len),
        Some(1),
        "the whole conversation crosses, including system messages"
    );
    // Not a member of pi's type and not on the wire: the host passes it as its own export
    // argument, so a decoded request carries the empty default until the export body fills it in.
    assert_eq!(request.route_id, "");
}

#[test]
fn an_absent_optional_stays_absent_rather_than_decoding_as_present() {
    let request: ModelRouteRequest = serde_json::from_value(json!({
        "model": { "provider": "router", "id": "demo-auto" },
        "thinkingLevel": "low",
        "reason": "user",
        "messages": []
    }))
    .expect("the three required members alone must decode");
    assert!(request.previous.is_none());
    assert!(request.failed.is_none());
    assert!(
        request.state.is_none(),
        "pi documents state as absent before the first one and for every `direct` request; a \
         router that distinguishes them needs the absence, not a null"
    );
}

#[test]
fn a_route_serializes_to_pis_model_route_and_omits_an_unset_state() {
    let route = ModelRoute::new(json!({ "provider": "faux", "id": "large" }), "high");
    let wire: Value = serde_json::to_value(&route).unwrap();
    assert_eq!(wire.get("thinkingLevel"), Some(&json!("high")));
    assert!(
        wire.get("state").is_none(),
        "pi: \"Return `request.state` or undefined to keep the current state\" — an unset state \
         must be ABSENT, not null, so the host's unchanged test behaves as upstream's"
    );

    let kept = route.with_state(json!({ "phase": "plan" }));
    let wire: Value = serde_json::to_value(&kept).unwrap();
    assert_eq!(wire.get("state"), Some(&json!({ "phase": "plan" })));
}

#[test]
fn a_spec_serializes_in_pis_shape_with_omitted_optionals_omitted() {
    let bare: Value =
        serde_json::to_value(VirtualModelSpec::new("router", "demo-auto", "Demo")).unwrap();
    assert_eq!(
        bare,
        json!({ "provider": "router", "id": "demo-auto", "name": "Demo" }),
        "an omitted optional must not reach the host as null: the host applies pi's defaults \
         (`thinkingLevels ?? [\"off\"]`) on ABSENCE, and an explicit null would decode as a \
         present-but-empty value instead"
    );

    let full: Value = serde_json::to_value(
        VirtualModelSpec::new("router", "demo-auto", "Demo")
            .with_thinking_levels(["low", "high"])
            .with_limits(200_000, 8_192),
    )
    .unwrap();
    assert_eq!(full.get("thinkingLevels"), Some(&json!(["low", "high"])));
    assert_eq!(full.get("contextWindow"), Some(&json!(200_000)));
    assert_eq!(full.get("maxTokens"), Some(&json!(8_192)));
}

fn echo_router(level: &'static str) -> impl VirtualModelRouter {
    move |_r: &ModelRouteRequest, _c: &Ctx| {
        Ok(ModelRoute::new(
            json!({ "provider": "faux", "id": "large" }),
            level,
        ))
    }
}

#[test]
fn registering_the_same_pair_again_replaces_it_in_place() {
    let mut api = ExtensionApi::new();
    api.register_virtual_model(
        VirtualModelSpec::new("router", "a", "A"),
        echo_router("low"),
    );
    api.register_virtual_model(
        VirtualModelSpec::new("router", "b", "B"),
        echo_router("low"),
    );
    api.register_virtual_model(
        VirtualModelSpec::new("router", "a", "A-again"),
        echo_router("high"),
    );

    let specs = api.virtual_model_specs();
    assert_eq!(
        specs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"],
        "pi's `Map.set` on an existing key keeps its POSITION, and the listing order is \
         observable (`test/virtual-models.test.ts:174-175`)"
    );
    assert_eq!(
        specs.first().map(|s| s.name.as_str()),
        Some("A-again"),
        "the replacement must be what crosses the seam, not the original"
    );

    let request: ModelRouteRequest = serde_json::from_value(json!({
        "model": {}, "thinkingLevel": "low", "reason": "user", "messages": []
    }))
    .unwrap();
    let route = api
        .route_virtual_model("router", "a", &request, &Ctx::new())
        .expect("the replacement's router answers");
    assert_eq!(
        route.thinking_level, "high",
        "the ROUTER was replaced too, not only the spec"
    );
}

#[test]
fn routing_a_pair_this_guest_never_registered_is_an_error_not_a_route() {
    let mut api = ExtensionApi::new();
    api.register_virtual_model(
        VirtualModelSpec::new("router", "a", "A"),
        echo_router("low"),
    );
    let request: ModelRouteRequest = serde_json::from_value(json!({
        "model": {}, "thinkingLevel": "low", "reason": "user", "messages": []
    }))
    .unwrap();

    let err = api
        .route_virtual_model("router", "nope", &request, &Ctx::new())
        .expect_err(
            "an unexpected call must be a FAILED route: a fabricated successful answer would be \
             streamed, which is the one outcome upstream's contract rules out",
        );
    assert!(
        err.contains("router/nope"),
        "the message names the pair: {err}"
    );
}

#[test]
fn unregistering_drops_only_that_pair() {
    let mut api = ExtensionApi::new();
    api.register_virtual_model(
        VirtualModelSpec::new("router", "a", "A"),
        echo_router("low"),
    );
    api.register_virtual_model(
        VirtualModelSpec::new("router", "b", "B"),
        echo_router("low"),
    );

    assert!(api.unregister_virtual_model("router", "a"));
    assert!(
        !api.unregister_virtual_model("router", "a"),
        "pi's absent-pair arm is a no-op, not a second removal"
    );
    assert_eq!(
        api.virtual_model_specs()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b"],
        "a sibling under the SAME provider id survives — upstream's unregister is per-pair \
         (`model-runtime.ts:976-982`)"
    );
}

#[test]
fn the_demo_guest_registers_its_virtual_model_through_the_same_surface() {
    let api = crate::example::build();
    let specs = api.virtual_model_specs();
    let demo = specs
        .iter()
        .find(|s| s.provider == "router" && s.id == "demo-auto")
        .expect(
            "the one fixture guest in the tree must register a virtual model, or no integration \
             target can exercise a guest router at all",
        );
    assert_eq!(
        demo.thinking_levels.as_deref(),
        Some(&["low".to_string(), "high".to_string()][..])
    );
}

//! The demo extension's VIRTUAL MODEL — the guest tier of pi `pi.registerVirtualModel()`
//! (`core/extensions/types.ts:1865-1872` @v1.0.4), live.
//!
//! A virtual model is a selectable catalog entry that routes each request to a physical model: the
//! selection names `router/demo-auto`, and each assistant message records the physical model and
//! thinking level [`install`]'s router picked. The spec crosses at `init` through
//! `registration.register-virtual-model`; the router itself stays HERE, behind the `route-model`
//! export the host calls once per provider request, because ADR-0002 keeps callables off the value
//! seam.
//!
//! The router reproduces upstream's own test router verbatim in behaviour
//! (`test/suite/virtual-models.test.ts:19-30` @v1.0.4, `defaultRoute`): `direct` goes to the big
//! model at a cheap level, any non-`user` reason with a `failed ?? previous` STAYS on that physical
//! model, and a fresh user turn picks by thinking level. It additionally returns router state —
//! upstream's phased example does (`docs/virtual-models.md`) — so the state round trip is exercised
//! across the component boundary rather than only inside one process.

use serde_json::{Value, json};

use crate::{
    CommandDescriptor, ExtensionApi, ModelRoute, ModelRouteRequest, ToolCall, ToolDescriptor,
    ToolOutput, VirtualModelSpec,
};

/// The provider id the demo virtual model is listed under. A provider of ONLY virtual models, which
/// upstream explicitly allows ("`provider` may be any provider id, including one with physical
/// models, and may list several virtual models").
pub const DEMO_ROUTER_PROVIDER: &str = "router";

/// The demo virtual model's id.
pub const DEMO_ROUTER_ID: &str = "demo-auto";

/// Install the demo virtual model and its router.
pub fn install(api: &mut ExtensionApi) {
    api.register_virtual_model(
        VirtualModelSpec::new(DEMO_ROUTER_PROVIDER, DEMO_ROUTER_ID, "Demo Auto Router")
            // pi `thinkingLevels` — the levels the model picker offers for this entry. Two, so the
            // "pick by level" arm below is reachable from the UI.
            .with_thinking_levels(["low", "high"])
            // pi `contextWindow` / `maxTokens`: shown only BEFORE the first response; afterwards
            // the limits of the physical model that answered apply.
            .with_limits(200_000, 8_192),
        route,
    );
    install_instance_hold(api);
}

/// The name of the tool that HOLDS this guest's instance, and of the command that does the same.
///
/// A WASM instance runs one call at a time, which is the one hazard a guest router has that a pi
/// router does not: if a route arrives while the instance is inside another call, queueing it
/// behind suspended wasm can wait indefinitely. The host answers with two bounds —
/// `GuestReentry::RouterOfBusyInstance` when a TOOL holds the instance, and a bounded wait on the
/// instance mutex otherwise — and neither can be tested without a guest that really holds it.
///
/// Both spin rather than sleep because the guest SDK has no sleep: holding the instance is the
/// point, and a spin holds it exactly as a slow backend call would.
pub const HOLD_TOOL: &str = "vm_hold";

/// What the demo router notifies through `ui.notify` the moment its `is_cancelled()` poll answers
/// true, so a test can assert the GUEST observed pi's `request.signal` rather than inferring it from
/// which side of the host's `select!` happened to win.
pub const CANCEL_OBSERVED: &str = "demo router: cancellation observed";

/// The slash command half of [`HOLD_TOOL`] — it holds the instance WITHOUT an in-flight tool call,
/// which is the case the host's bounded mutex wait covers rather than its reentrancy refusal.
pub const HOLD_COMMAND: &str = "vm-hold";

fn install_instance_hold(api: &mut ExtensionApi) {
    api.register_tool(
        ToolDescriptor::new(
            HOLD_TOOL,
            json!({
                "type": "object",
                "properties": { "ms": { "type": "integer" } }
            }),
        )
        .description("Hold this extension's wasm instance for `ms` milliseconds (demo)."),
        |call: ToolCall| {
            let ms = call.params.get("ms").and_then(Value::as_u64).unwrap_or(50);
            spin(ms);
            Ok(ToolOutput::text(format!("held {ms}ms")))
        },
    );
    api.register_command(
        HOLD_COMMAND,
        CommandDescriptor::new("Hold this extension's wasm instance (demo)."),
        |args: &str, _ctx: &crate::CommandCtx| {
            let ms = args.trim().parse::<u64>().unwrap_or(50);
            spin(ms);
            Ok(Some(format!("held {ms}ms")))
        },
    );
}

/// Busy-hold the instance for `ms`, capped well under the host's ~5s epoch budget so the hold ends
/// as a RETURN rather than as a trap.
fn spin(ms: u64) {
    let budget = std::time::Duration::from_millis(ms.min(2_000));
    let started = std::time::Instant::now();
    while started.elapsed() < budget {
        std::hint::spin_loop();
    }
}

/// The demo router — pi's `ExtensionVirtualModel.route(request, ctx)`.
///
/// Returns `Err` for an unroutable request rather than guessing: upstream's contract is that a
/// throwing `route()` ends the request with an error response, and a guessed physical model would
/// be streamed instead.
fn route(request: &ModelRouteRequest, ctx: &crate::Ctx) -> Result<ModelRoute, String> {
    // pi's `request.signal.aborted`, as the `route-id`-keyed poll a `CancelToken` becomes to cross
    // the boundary. A router that already knows it is cancelled stops immediately.
    if request.is_cancelled() {
        return Err("demo router: the request was cancelled before it routed".to_string());
    }
    // And the other half of pi's guidance, which is the half a router doing real work needs: POLL
    // between units of work. A request carrying `{"awaitCancel": true}` in its router state makes
    // the demo do exactly that, so the poll is exercised while the guest is running rather than
    // only at entry. The observation is reported through `ui.notify` as well as returned, because
    // which side of the host's `select!` wins the race is the host's business — the notification is
    // what proves the GUEST saw it.
    if request
        .state
        .as_ref()
        .and_then(|s| s.get("awaitCancel"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1_500);
        while std::time::Instant::now() < deadline {
            if request.is_cancelled() {
                ctx.ui().notify(CANCEL_OBSERVED);
                return Err("demo router: cancelled while it was working".to_string());
            }
            spin(5);
        }
        return Err("demo router: no cancellation arrived while it waited".to_string());
    }

    // pi's `const find = (id) => ctx.modelRegistry.find("faux", id)!`. The model is returned WHOLE
    // because that is what a `ModelRoute` carries — the host re-resolves the address against its
    // own catalog, and the full object decodes on every path.
    let any_physical = |want_big: bool| -> Option<Value> {
        let models = ctx.models().list();
        let rows = models.as_array()?;
        // Biggest / smallest by context window, so the demo needs no hard-coded provider id and
        // works against whatever catalog the host composed.
        let pick = |a: &&Value, b: &&Value| {
            let w = |m: &Value| m.get("contextWindow").and_then(Value::as_u64).unwrap_or(0);
            w(a).cmp(&w(b))
        };
        let chosen = if want_big {
            rows.iter().max_by(pick)
        } else {
            rows.iter().min_by(pick)
        };
        chosen.cloned()
    };

    // `if (request.reason === "direct") return { model: find("large"), thinkingLevel: "low" }`.
    // A `direct` request is a compaction summary or an extension call; its returned state is
    // ignored by the host, which is why none is set here.
    if request.reason == "direct" {
        let model = any_physical(true)
            .ok_or_else(|| "demo router: no physical model to summarize with".to_string())?;
        return Ok(ModelRoute::new(model, "low"));
    }

    // `const sticky = request.failed ?? request.previous; if (request.reason !== "user" && sticky)`
    // — a continuation or a retry STAYS on the physical model that last answered, at its level.
    // This is the arm that proves `previous`/`failed` crossed the boundary with real values.
    let sticky = request
        .failed
        .as_ref()
        .map(|f| (f.model.clone(), f.thinking_level.clone()))
        .or_else(|| {
            request
                .previous
                .as_ref()
                .map(|p| (p.model.clone(), p.thinking_level.clone()))
        });
    if request.reason != "user"
        && let Some((model, level)) = sticky
    {
        return Ok(
            ModelRoute::new(model, level.unwrap_or_else(|| "high".into()))
                .with_state(next_state(request, "sticky")),
        );
    }

    // `request.thinkingLevel === "high" ? find("large")@high : find("small")@off`.
    let want_big = request.thinking_level == "high";
    let model = any_physical(want_big)
        .ok_or_else(|| "demo router: no physical model to route to".to_string())?;
    let level = if want_big { "high" } else { "off" };
    Ok(ModelRoute::new(model, level).with_state(next_state(request, "fresh")))
}

/// The router state this demo keeps: a turn counter plus the arm that last decided, so a test can
/// see both that the state ARRIVED (`request.state`) and that a new one was stored.
///
/// pi's rule, honoured by the host rather than here: a state equal to `request.state` stores
/// nothing, and a `direct` request's state is ignored.
fn next_state(request: &ModelRouteRequest, phase: &str) -> Value {
    let turns = request
        .state
        .as_ref()
        .and_then(|s| s.get("turns"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({ "turns": turns + 1, "phase": phase })
}

//! A worked virtual-model router, shipped as a compiled-in native extension behind an env gate.
//!
//! This is cyrup's analogue of upstream's `packages/coding-agent/examples/extensions/jev-router.ts`
//! (@v1.0.4) and of the shorter router in `docs/virtual-models.md`'s "Register a virtual model"
//! section. It exists because the virtual-model feature is otherwise UNREACHABLE from the shipped
//! binary: cyrup has no `examples/` directory, natives are compile-time
//! `Arc<dyn NativeExtension>` values handed to [`cyrup_session_svc::SessionFactory`], and the WASM
//! guest tier has no `register-virtual-model` import yet (see this module's "Not ported" note).
//!
//! # What it routes
//!
//! Registers `router/auto`, offering the thinking levels `low` and `high` — `provider: "router"`,
//! `id: "auto"` is the docs page's own example pair. Its rules are the STRUCTURE of
//! `jev-router.ts`, with the Jev classifier call deliberately left out (see below):
//!
//! 1. `direct` — a request outside the agent loop, such as a compaction summary — goes to the small
//!    model, as `jev-router.ts:102` routes `direct` to a fixed cheap model.
//! 2. Any non-`user` request with a sticky model — `request.failed ?? request.previous` — stays on
//!    it, at the level that turn was answered with (`?? medium`). This is the docs page's example
//!    verbatim, and it is what keeps prompt caches and thinking signatures valid across tool
//!    follow-ups and retries.
//! 3. Otherwise the SELECTED virtual level picks the model: `high` → the large model, anything else
//!    → the small one.
//!
//! It also keeps a turn counter as router state (`{"turns": n}`), which exercises the
//! `pi.virtual-model-state` round trip end to end — the entry is written on the session branch, so
//! it follows forks and `/tree` navigation and survives compaction.
//!
//! # Enabling it
//!
//! `CYRUP_ROUTER_EXAMPLE=1` (or any value that is not `0`/`false`/empty). The gate defaults OFF, so
//! no existing session's catalog changes: a virtual model appears in `/model` only when the user
//! asked for one. `CYRUP_ROUTER_EXAMPLE_SMALL` / `CYRUP_ROUTER_EXAMPLE_LARGE` name the two targets
//! as `provider/id`; unset, the router picks the smallest- and largest-context models the session's
//! provider lists, so it works with no further configuration.
//!
//! # Not ported, deliberately
//!
//! * The Jev classifier call (`jev-router.ts:62-87`, `ctx.modelRegistry.classify()` over a
//!   `findOfType("classifier", …)` model). That is a separate unported surface and porting it here
//!   would widen this step's scope.
//! * The WASM/guest tier. A guest router is a guest CLOSURE, so it needs a
//!   `registration.register-virtual-model` import pair AND a new ASYNC guest export
//!   (`events.route-virtual-model`) — the same inversion `tool-descriptor.prepare-arguments` and
//!   `events.bash-operations-exec` already have, and a new export forces a WIT world minor bump
//!   under `check_world`'s own rule. Same class as the open CODE-015/CODE-016 rows; it needs a row
//!   of its own.

use std::sync::Arc;

use cyrup_core::{ExtensionId, ModelThinkingLevel, ProviderId};
use cyrup_ext::contract::HookOutcome;
use cyrup_ext::error::ExtError;
use cyrup_ext::event::HostEvent;
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_provider::{
    Model, ModelRoute, ModelRouteError, ModelRouteReason, ModelRouteRequest, ModelRouter,
    VirtualModelDefinition, VirtualModelSpec,
};

/// The extension id, in the `builtin:` namespace the other compiled-in natives use.
pub const EXTENSION_ID: &str = "builtin:router-example";

/// The env gate. OFF unless set to something other than `0` / `false` / empty.
const GATE_ENV: &str = "CYRUP_ROUTER_EXAMPLE";
/// Optional `provider/id` override for the cheap target.
const SMALL_ENV: &str = "CYRUP_ROUTER_EXAMPLE_SMALL";
/// Optional `provider/id` override for the capable target.
const LARGE_ENV: &str = "CYRUP_ROUTER_EXAMPLE_LARGE";

/// The provider id the example virtual model is listed under (docs page's own example).
pub const ROUTER_PROVIDER: &str = "router";
/// The example virtual model's id (docs page's own example).
pub const ROUTER_MODEL_ID: &str = "auto";

/// Whether the gate value turns the example on. Pure, so it is testable in this crate — which is
/// `#![forbid(unsafe_code)]` and therefore cannot reach `std::env::set_var` from a test.
#[must_use]
pub fn gate_enabled_from(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        }
        None => false,
    }
}

/// Whether the gate is on. One predicate, read by the attach site.
#[must_use]
pub fn gate_enabled() -> bool {
    gate_enabled_from(std::env::var(GATE_ENV).ok().as_deref())
}

/// The bundled example router, or `None` when the gate is off — the same shape
/// `cyrup_llama::llama_extension_for_env` has, so the attach site reads the same way for both.
#[must_use]
pub fn router_extension_for_env() -> Option<Arc<dyn NativeExtension>> {
    gate_enabled().then(|| Arc::new(RouterExampleExtension::new()) as Arc<dyn NativeExtension>)
}

/// Parse a `provider/id` override. The id may itself contain `/` (OpenRouter-style), so the split
/// is on the FIRST separator only.
fn parse_target(spec: &str) -> Option<(String, String)> {
    let (provider, id) = spec.trim().split_once('/')?;
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider.to_string(), id.to_string()))
}

/// The two targets a request may be routed to.
#[derive(Debug)]
struct Targets {
    small: Model,
    large: Model,
}

/// Resolve an explicit `provider/id` override against the catalog.
fn by_spec(catalog: &[Model], spec: Option<&str>) -> Option<Model> {
    let (provider, id) = parse_target(spec?)?;
    catalog
        .iter()
        .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
        .cloned()
}

/// The two targets: the `provider/id` overrides when they resolve, else the smallest- and
/// largest-context models the catalog lists.
///
/// `min_by_key`/`max_by_key` over `context_window` is a HEURISTIC, not a port of anything — the
/// point of the default is that the example works with no configuration. It is named here so a
/// reader is not misled into thinking upstream does this; `jev-router.ts` hard-codes three ids.
fn targets(
    catalog: &[Model],
    small_spec: Option<&str>,
    large_spec: Option<&str>,
) -> Result<Targets, ModelRouteError> {
    // A virtual model must never be a target: "A virtual model cannot route to another virtual
    // model", and `resolve_model` rejects such a route outright. Filtering here keeps the example
    // from producing a route the registry would only refuse.
    let physical: Vec<&Model> = catalog
        .iter()
        .filter(|m| !cyrup_provider::is_virtual_model(m))
        .collect();
    let Some(smallest) = physical.iter().min_by_key(|m| m.context_window) else {
        return Err(ModelRouteError::new(
            "router/auto has no physical models to route to",
        ));
    };
    let largest = physical
        .iter()
        .max_by_key(|m| m.context_window)
        .unwrap_or(smallest);
    Ok(Targets {
        small: by_spec(catalog, small_spec).unwrap_or_else(|| (*smallest).clone()),
        large: by_spec(catalog, large_spec).unwrap_or_else(|| (*largest).clone()),
    })
}

/// The whole routing decision, as a pure function of the two overrides and the request — the
/// catalog comes off the request itself (`request.catalog`), which is pi's `ctx.modelRegistry`
/// (`jev-router.ts:41`). `AutoRouter::route` is this plus the two env reads.
fn choose(
    small_spec: Option<&str>,
    large_spec: Option<&str>,
    request: &ModelRouteRequest<'_>,
) -> Result<ModelRoute, ModelRouteError> {
    let catalog = request.catalog.models();
    let Targets { small, large } = targets(&catalog, small_spec, large_spec)?;
    // The turn counter, bumped on every routed request. Absent on the first request of a branch,
    // which is pi's `request.state ?? …`.
    let turns = request
        .state
        .and_then(|s| s.get("turns"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let next_state = serde_json::json!({ "turns": turns + 1 });

    // 1. `if (request.reason === "direct") return routeTo(request, ctx, LUNA)`
    //    (`jev-router.ts:102`). A direct request has no state and any state it returns is
    //    IGNORED, so none is produced here — returning one would be dead weight the session
    //    discards.
    if request.reason == ModelRouteReason::Direct {
        return Ok(ModelRoute {
            model: small,
            thinking_level: request.thinking_level,
            state: None,
        });
    }

    // 2. The sticky rule, verbatim from `docs/virtual-models.md`'s example:
    //      const sticky = request.failed ?? request.previous;
    //      if (request.reason !== "user" && sticky)
    //        return { model: sticky.model, thinkingLevel: sticky.thinkingLevel ?? "medium" };
    //    `failed` first: a retry must go back to the model whose request failed, not to the model
    //    of the turn before it.
    let sticky = request
        .failed
        .as_ref()
        .map(|f| (&f.model, f.thinking_level))
        .or_else(|| {
            request
                .previous
                .as_ref()
                .map(|p| (&p.model, p.thinking_level))
        });
    if request.reason != ModelRouteReason::User
        && let Some((model, level)) = sticky
    {
        return Ok(ModelRoute {
            model: model.clone(),
            thinking_level: level.unwrap_or(ModelThinkingLevel::Medium),
            state: Some(next_state),
        });
    }

    // 3. `const id = request.thinkingLevel === "high" ? <big> : <small>`.
    let model = if request.thinking_level == ModelThinkingLevel::High {
        large
    } else {
        small
    };
    Ok(ModelRoute {
        model,
        thinking_level: request.thinking_level,
        state: Some(next_state),
    })
}

/// The router: [`choose`] over the catalog the request carries.
///
/// It holds NO state of its own. It used to hold the session's `HostServices` purely to read the
/// model catalog, because `ModelRouteRequest` carried none — the gap PROV-DEFECT-4 closed by
/// putting pi's per-request `ctx.modelRegistry` on the request as
/// [`ModelRouteRequest::catalog`]. A third-party router therefore needs no back door either.
struct AutoRouter;

#[async_trait::async_trait]
impl ModelRouter for AutoRouter {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        let small = std::env::var(SMALL_ENV).ok();
        let large = std::env::var(LARGE_ENV).ok();
        choose(small.as_deref(), large.as_deref(), &request)
    }
}

/// The native extension that registers [`AutoRouter`] as `router/auto`.
struct RouterExampleExtension {
    router: Arc<AutoRouter>,
}

impl RouterExampleExtension {
    fn new() -> Self {
        Self {
            router: Arc::new(AutoRouter),
        }
    }
}

/// The registration this extension makes, built as a free function so a test can assert its shape
/// without loading an extension host.
fn definition(router: Arc<dyn ModelRouter>) -> VirtualModelDefinition {
    VirtualModelDefinition::new(
        VirtualModelSpec {
            provider: ProviderId::from(ROUTER_PROVIDER),
            id: ROUTER_MODEL_ID.into(),
            name: "Auto".to_string(),
            // The docs page's example offers exactly these two.
            thinking_levels: Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]),
            // Deliberately unset: "`contextWindow` and `maxTokens` are shown before the first
            // response. Unset limits are unknown." Once a response lands, the limits follow the
            // PHYSICAL model that answered, which is what makes leaving these out correct rather
            // than lazy.
            context_window: None,
            max_tokens: None,
            input: None,
        },
        router,
    )
}

#[async_trait::async_trait]
impl NativeExtension for RouterExampleExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(EXTENSION_ID)
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_virtual_model(definition(Arc::clone(&self.router) as Arc<dyn ModelRouter>));
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    /// Ambient, like the other compiled-in natives: it stands in for an installed package in the
    /// tier `--no-extensions` collapses, so that flag drops it too.
    fn is_ambient(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;

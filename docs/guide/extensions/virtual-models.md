# Virtual models

A **virtual model** is a selectable model that picks a physical model for each request. Use one to
route by task, cost, or conversation state — a router can send quick questions to a small model and
hard problems to a large one while the user selects a single model.

Virtual models appear in `/model`, `--model`, scoped models, and settings like any other model. A
virtual model can be listed under any provider, including one that already has physical models.

Both extension tiers can register one: a compiled-in **native** extension (and any embedder) through
`InitApi::register_virtual_model`, and a **WebAssembly** extension through the guest SDK's
`ExtensionApi::register_virtual_model`. The two go through the same registry, so they behave
identically; where the guest tier differs, this page says so.

## Selection and dispatch

A virtual model selects a model and a thinking level. A router maps that pair to a physical pair for
each request:

```text
selected (virtual model, virtual level)  ->  dispatched (physical model, physical level)
router/auto:low                          ->  anthropic/claude-sonnet-4-5:high
```

The virtual thinking level is an input to the router. Its meaning is up to the router; it need not
correspond to a reasoning budget.

cyrup keeps the two pairs apart:

| | Selection | Dispatch |
|---|---|---|
| Recorded in | `model_change` and `thinking_level_change` entries | Each assistant message: `provider`, `api`, `model`, `thinkingLevel` |
| Visible as | `ctx.model`, `ctx.thinkingLevel`, `/model` | The assistant message of each response |

Providers only ever receive physical models. Assistant messages name the physical model, so
replaying a conversation across different physical models works the same as after a manual model
switch. Resuming a session restores the virtual selection from its latest `model_change` entry; if
the virtual model is no longer registered, cyrup falls back to the physical model that answered
last.

In the terminal interface the footer shows the routed model next to the selection, for example
`auto • high → anthropic/claude-sonnet-4-5 • medium`. `/session` lists the cost for each physical
model.

Context usage uses the limits of the physical model that produced the latest response, even if that
response came before switching to the virtual model. Without such a response it uses the limits
declared on the virtual model, if any. Compaction checks the same limits, and again the limits of
the model each request is routed to: if that model's context window is too small for the
conversation, cyrup compacts before sending the request, and the route stays as the router chose it.

## Register a virtual model

```rust
use std::sync::Arc;

use cyrup_ext::native::{InitApi, NativeExtension};
use cyrup_provider::{
    ModelRoute, ModelRouteError, ModelRouteReason, ModelRouteRequest, ModelRouter,
    VirtualModelDefinition, VirtualModelSpec,
};

struct Auto;

#[async_trait::async_trait]
impl ModelRouter for Auto {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        // Tool follow-ups and retries stay on the model that handled the turn.
        let sticky = request.failed.as_ref().map(|f| (&f.model, f.thinking_level))
            .or_else(|| request.previous.as_ref().map(|p| (&p.model, p.thinking_level)));
        if request.reason != ModelRouteReason::User && let Some((model, level)) = sticky {
            return Ok(ModelRoute {
                model: model.clone(),
                thinking_level: level.unwrap_or(cyrup_core::ModelThinkingLevel::Medium),
                state: None,
            });
        }
        // ... pick a physical model and return it
        Err(ModelRouteError::new("not implemented"))
    }
}

// inside `NativeExtension::init`
async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
    api.register_virtual_model(VirtualModelDefinition::new(
        VirtualModelSpec {
            provider: "router".into(),
            id: "auto".into(),
            name: "Auto".to_string(),
            thinking_levels: Some(vec![
                cyrup_core::ModelThinkingLevel::Low,
                cyrup_core::ModelThinkingLevel::High,
            ]),
            context_window: None,
            max_tokens: None,
            input: None,
        },
        Arc::new(Auto),
    ));
    Ok(())
}
```

- `provider` is the provider the model is listed under. It can be any provider id. A provider can
  list several virtual models next to its physical ones. On a physical provider, the virtual model is
  available when that provider has credentials. Under an id no provider uses, it is always available.
- `id` must not be the id of a physical model of that provider. If a catalog refresh later adds a
  physical model with the same id, the virtual model hides it.
- `thinking_levels` lists the levels offered for selection. It defaults to `["off"]`.
- `context_window` and `max_tokens` are shown before the first response. Unset limits are unknown.
- `input` lists the input types offered for selection. It defaults to text and images.

Registration follows the same queuing and reload rules as `register_provider_live`: a registration
made during `init` is queued and flushed once the session's model registry exists, and one made
afterwards through the `LateRegistrar` takes effect immediately. Registering the same provider and
id again **replaces** the virtual model. `LateRegistrar::unregister_virtual_model(provider, id)`
removes it; unregistering the *provider* does not. Embedders can register without an extension at
all, through `AgentSession::register_virtual_model` or by handing `SessionBuilder::virtual_models` a
populated registry before the session opens.

A registration the registry refuses — an empty provider or id, or an id that already names a
physical model of that provider — becomes a startup diagnostic and does not fail the extension load.

## Register one from a WebAssembly extension

The guest SDK mirrors the native surface. `route` stays inside the guest — the host calls it through
the `events.route-model` export once per request — so only the spec crosses the boundary:

```rust
use cyrup_ext_sdk::prelude::*;
use serde_json::json;

fn build() -> ExtensionApi {
    let mut api = ExtensionApi::new();
    api.register_virtual_model(
        VirtualModelSpec::new("router", "auto", "Auto")
            .with_thinking_levels(["low", "high"])
            .with_limits(200_000, 8_192),
        |request: &ModelRouteRequest, ctx: &Ctx| {
            // Tool follow-ups and retries stay on the model that handled the turn.
            let sticky = request.failed.as_ref().map(|f| (&f.model, &f.thinking_level))
                .or_else(|| request.previous.as_ref().map(|p| (&p.model, &p.thinking_level)));
            if request.reason != "user" && let Some((model, level)) = sticky {
                let level = level.clone().unwrap_or_else(|| "medium".into());
                return Ok(ModelRoute::new(model.clone(), level));
            }
            // Name the physical model the way pi's docs do, through the model registry.
            let model = ctx.models().find("anthropic", "claude-sonnet-4-5")
                .ok_or_else(|| "no such model".to_string())?;
            Ok(ModelRoute::new(model, "high").with_state(json!({ "turns": 1 })))
        },
    );
    api
}

cyrup_ext_sdk::export_extension!(build);
```

Differences from the native tier, all of them consequences of the component boundary:

- **Name the model through `ctx.models().find(provider, id)`.** That is the guest's view of the model
  registry (`models.list-models`), and returning the whole object it gives you is what a router is
  documented to do. A `{"provider": …, "id": …}` object works too — the host resolves it against the
  catalog either way, because a router's model is only an address.
- **`request.is_cancelled()` replaces `signal`.** A cancellation token is not a value a component can
  carry, so the host binds this request's token for the duration of the call and the guest polls it.
  Check it between units of work if your router does I/O, and return promptly once it is true.
- **A busy instance refuses the route rather than queueing behind it.** A WebAssembly instance runs
  one call at a time. If a tool of the same extension is executing when a request needs routing, the
  route is refused by name and the request ends with an error response naming the tool that held the
  instance. Keep a router independent of your extension's tools, and do not route from inside one.
- **The wait for the instance is bounded.** If the instance is busy for another reason, the host gives
  up after a few seconds rather than holding the turn, and the request ends with an error response.
- **The whole conversation crosses on every request.** `request.messages` is the full transcript,
  copied into the guest, which has a 64 MiB memory cap. A router that keeps per-request copies of a
  long transcript can exhaust it; a growth past the cap is contained as a failed route, not a crash.
- **A trap or a runaway loop is contained.** The call runs under the extension epoch budget; a guest
  that traps or loops past it ends the request with an error response and leaves the host and the
  instance usable.

Everything else is the same: registration is queued and flushed exactly as `register_provider` is,
re-registering a pair replaces it, `Ctx::register_virtual_model` registers one from a live handler,
`Ctx::unregister_virtual_model(provider, id)` removes one, unregistering the *provider* does not, and
unloading the extension drops its virtual models.

A WebAssembly extension that registers a virtual model needs `"world": "cyrup:ext@0.20"` or newer in
its `extension.json`: the route callback is a guest export, and a component built against an older
world does not have it.

## Route requests

`route(request)` runs before every request made with the virtual model and returns a model and a
thinking level. The model can be any physical model in the catalog whose provider has credentials. A
virtual model cannot route to another virtual model. cyrup clamps the thinking level to the returned
model.

| Field | Meaning |
|---|---|
| `model`, `thinking_level` | The selected virtual model and level |
| `reason` | Why the request is made, see below |
| `previous` | Physical model and thinking level of the latest successful response in `messages` |
| `failed` | For `Retry`: physical model, thinking level, and assistant `message` of the failed request, which `messages` no longer contains. Absent when routing itself failed |
| `state` | Router state last returned on this session branch, see below |
| `messages` | The conversation for this request, including system messages |
| `cancel` | Cancellation token of the request |

| `reason` | Request |
|---|---|
| `User` | First request after a message the user wrote, including steering and follow-up messages |
| `Continuation` | Any other request in the agent loop, such as after tool results or extension messages |
| `Retry` | Automatic retry after a failed request, including after compaction for a context overflow |
| `Direct` | Request made outside the agent loop, such as a compaction summary |

Returning `previous` for `Continuation` and `failed` for `Retry` keeps prompt caches and thinking
signatures valid. Switching models between turns is allowed but loses the prompt cache. A retry can
also switch to another model, for example when `failed.message.error_message` reports that a
provider is overloaded or the context overflowed.

If `route()` returns an error, or returns a virtual model or a model without credentials, the
request ends with an error response naming the virtual model.

## Keep routing state

`route()` can return `state` next to the model. cyrup stores it on the session branch and passes it
back as `request.state` on later requests. Use it for decisions the transcript does not record, such
as classifier results or a routing phase.

- State must be JSON (`serde_json::Value`).
- Returning `None`, or the same value as `request.state`, keeps the current state.
- Any other value is stored as new state, before the request is sent. The state stays stored if the
  request later fails.
- State follows the session tree, so forks and `/tree` navigation see the state of their branch. It
  survives compaction.
- `Direct` requests have no state, and cyrup ignores state they return.

> **One difference from pi.** pi compares the returned state to the current one by object IDENTITY,
> so a router that returns a structurally identical fresh object each turn writes a duplicate entry.
> Rust has no stable identity for a returned `serde_json::Value`, so cyrup compares by VALUE and
> writes nothing in that case. Nothing observable depends on the duplicate.

## A worked router

cyrup ships one, compiled in and gated off: set `CYRUP_ROUTER_EXAMPLE=1` to register `router/auto`,
which routes `high` to the largest-context model the session's provider lists and everything else to
the smallest, keeps tool follow-ups and retries on the model that handled the turn, sends compaction
summaries to the small model, and keeps a turn counter as router state.
`CYRUP_ROUTER_EXAMPLE_SMALL` / `CYRUP_ROUTER_EXAMPLE_LARGE` name the two targets explicitly as
`provider/id`. The source is `crates/cyrup/src/router_example.rs`.

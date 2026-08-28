---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# 13i — Sampling, Elicitation And The Protocol Tracer

## Objective

Build the three absent protocol surfaces of section 13i, and wire them so they are **reachable at
runtime**:

1. **`sampling/createMessage`** — a server asks the agent to run a completion; the human approves the
   request and the response; the server gets back a single text block.
2. **`elicitation/create`** — a server asks the human a typed, schema-validated question (form mode)
   or asks to open a URL (url mode).
3. **the protocol tracer** — a metadata-only JSONL record of every JSON-RPC message in both
   directions, bounded, redacted, and incapable of changing a byte on the wire.

Plus the one non-13i prerequisite without which all of the above is dead code: **a production
handler factory**, so the manager's hooks actually reach `McpClientHandler`.

This file was previously a *scoping* document whose deliverable was a plan. It is now the
implementation task. The triage it carried has been re-verified against the tree at
`2026-08-27` and **eight of its premises were wrong** (§1). The verification half of 13i
(`MCP-483`…`MCP-499`) is test/CI/doc work and is **out of scope for this task** — its corrected
findings are relayed in §8 and nothing in the Definition of Done depends on them.

## Sources

| what | where |
|---|---|
| upstream sampling handler (284 lines) | [tmp/pi-mcp-adapter/sampling-handler.ts](../../tmp/pi-mcp-adapter/sampling-handler.ts) |
| upstream elicitation handler (348 lines) | [tmp/pi-mcp-adapter/elicitation-handler.ts](../../tmp/pi-mcp-adapter/elicitation-handler.ts) |
| upstream tracer (306 lines) | [tmp/pi-mcp-adapter/mcp-trace.ts](../../tmp/pi-mcp-adapter/mcp-trace.ts) |
| upstream dual-dialect validator (69 lines) | [tmp/pi-mcp-adapter/json-schema-validator.ts](../../tmp/pi-mcp-adapter/json-schema-validator.ts) |
| upstream wiring (`init.ts:118-141`, `server-manager.ts:677-741, 800-824, 445-457, 1133-1167`) | [tmp/pi-mcp-adapter/init.ts](../../tmp/pi-mcp-adapter/init.ts) · [tmp/pi-mcp-adapter/server-manager.ts](../../tmp/pi-mcp-adapter/server-manager.ts) |
| the 50-unit section spec | [13i-mcp-protocol-and-verification.md](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) |
| the ledger (13i at `:931`-`:986`) | [13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| the crate | [crates/cyrup-mcp/src/](../../crates/cyrup-mcp/src/) |

Upstream is pinned at tag `v2.26.1` (`fafae21`); `sampling-handler.ts` is byte-identical at
`v2.25.0`, so its `file:line` citations resolve under either reading.

---

## 1. Corrections to the premises — verified against the tree, 2026-08-27

Every row below was re-checked by reading the named file. **Do not carry the old claims forward.**

| # | the earlier claim | verified truth | evidence |
|---|---|---|---|
| C-1 | citations point at `proxy.rs:1302`, `:1486`, `:3731-3737`, `:4510` | **`proxy.rs` no longer exists.** It is the directory module `proxy/` (14 files). Every `proxy.rs:NNNN` citation in the previous revision is dangling. | [proxy/env.rs:111](../../crates/cyrup-mcp/src/proxy/env.rs) `UrlElicitationAction`; `:295` the `ProxyEnv` seam method; [proxy/call.rs:860-871](../../crates/cyrup-mcp/src/proxy/call.rs) the three action messages |
| C-2 | `MCP-467`'s first act is a ruling: reuse `BrowserLauncher` or port `openUrl` | **There is no question.** `elicitation-handler.ts:13` imports the bare `open` package and `:336` calls `await open(params.url)`. `openUrl`/`execOpen` ([utils.ts:7-35](../../tmp/pi-mcp-adapter/utils.ts)) — the `$BROWSER`/per-platform dispatcher — is **never on the elicitation path**. `opener::open` via [oauth.rs:2396](../../crates/cyrup-mcp/src/oauth.rs) `OpenerLauncher` **is** the 1:1 port. No divergence, no ADR. | `elicitation-handler.ts:13,336` |
| C-3 | `MCP-462` needs an iteration site to be built first | rmcp already solved it and the shape is fixed: `ElicitationSchema.properties` is a **`BTreeMap`** (sorted — the silent bug) and `property_order: Option<Vec<String>>` carries wire order, filled on deserialize. | `rmcp-3.1.4/src/model/elicitation_schema.rs:1118-1151`, `:1169-1181` |
| C-4 | `MCP-465` is blocked on a `jsonschema` version bump | `jsonschema 0.46.9` is already the workspace pin and **already has** `should_validate_formats`. No bump. | [Cargo.toml:189](../../Cargo.toml); `jsonschema-0.46.9/src/options.rs:350` |
| C-5 | `MCP-451`'s `task` guard is portable | `CreateMessageRequestParams` has **no `task` field** in rmcp 3.1.4 — the guard is structurally unreachable, exactly like [owner.rs:814](../../crates/cyrup-mcp/src/owner.rs) `MESSAGE_TEXT_UNKNOWN_BLOCK`. Record the string, name why it has no throw site. | `rmcp-3.1.4/src/model.rs:2865-2897` |
| C-6 | `HA-1` is landed end to end, and `install_surface_sync` is fired from `proxy.rs:4510` | The **pieces** exist ([native.rs:768](../../crates/cyrup-ext/src/native.rs), [facade.rs:131](../../crates/cyrup-ext/src/facade.rs), [extension.rs:118,166,425,783](../../crates/cyrup-mcp/src/extension.rs), [registration.rs:1946,2021,2072](../../crates/cyrup-mcp/src/registration.rs)) but **`install_surface_sync` has zero callers** — `grep -rn install_surface_sync crates/` returns only its own definition and two doc references. HA-1 is built and unfired. Irrelevant to this task either way: no 13i unit touches it. | `grep -rn "install_surface_sync" crates/ --include=*.rs` |
| C-7 | G1 matches twelve files across five crates; G2's true count is 20 | **Eleven** files, five crates: `cyrup` 3, `cyrup-permission-system` 3, `cyrup-tools` 2, `cyrup-tui` **2** (not 3), `cyrup-provider` 1. Only `cyrup-it` sets `autotests = false`, so the true `[[test]]` count is **8 + 11 = 19** against a cap of 7. | `ls crates/*/tests/*.rs`; `grep -n autotests crates/*/Cargo.toml` |
| C-8 | `MCP-468`'s ledger row: "`McpClientHandler::new` is called only at `runtime.rs:1615` (a test)" | Stale. It is called from [runtime.rs:1941](../../crates/cyrup-mcp/src/runtime.rs) inside `bare_handler_factory`, which **is** the installed production default. The row's *conclusion* (nothing is wired) still holds; its evidence does not. | [runtime.rs:1940-1953](../../crates/cyrup-mcp/src/runtime.rs) |

Claims that **held** on re-check, and are load-bearing here:

* `MCP-455` is **implemented and unwired** — [owner.rs:604-818](../../crates/cyrup-mcp/src/owner.rs) carries the four
  literals, `SamplingApproval { auto_approve, has_ui, dialog }`, `confirm_sampling`'s three branches,
  both formatters and `message_text`. Its own module doc ([owner.rs:53-61](../../crates/cyrup-mcp/src/owner.rs))
  already states the move plan: `sampling.rs` re-exports it. **Do not rewrite it.**
* `MCP-471`'s `McpDialog` is real and un-bypassed — [owner.rs:522-597](../../crates/cyrup-mcp/src/owner.rs); the only
  `HostServices::{confirm,select}` calls in the crate are its own at `:577` and `:587`. It is missing
  an `input` arm and a `notify` arm.
* `MCP-469`'s registry is on the **manager**, not `state.rs` —
  [server_manager.rs:1224](../../crates/cyrup-mcp/src/server_manager.rs), `:2582`, `:2601`, `:2610`, cleared at
  `:2265` and `:2470`. The notice text and the hook producer are what is missing.
* The tracer is genuinely absent — `grep -rn "TracingTransport\|TraceWriter\|McpTraceEvent\|redact_trace"
  crates/` returns **one comment**, [server_manager.rs:1431-1436](../../crates/cyrup-mcp/src/server_manager.rs).
  Its settings half exists unconsumed: [config.rs:1617-1634](../../crates/cyrup-mcp/src/config.rs) `TraceSettings`,
  `:876` `ServerEntry::trace`, `:1267`/`:1273`/`:1279` the accessors, [dirs.rs:116](../../crates/cyrup-mcp/src/dirs.rs)
  `TRACE_DIR` + `:203` `trace_dir()`.
* `jsonschema` has **zero uses** in `cyrup-mcp` — the manifest at
  [Cargo.toml:125-131](../../crates/cyrup-mcp/Cargo.toml) already writes down the exact intent
  (`$schema` dispatch across draft-07 and 2020-12, `should_validate_formats(true)`).
* `cyrup-provider` has **zero uses outside `oauth.rs`/`credentials.rs`** in this crate, but the
  manifest edge exists and names sampling as its reason ([Cargo.toml:45-50](../../crates/cyrup-mcp/Cargo.toml)).

---

## 2. Wave 0 — a production hook bag. Everything else is unreachable without it.

### 2.1 What is true today

`ConnectionBuilder::new` installs `bare_handler_factory`
([runtime.rs:2283-2292](../../crates/cyrup-mcp/src/runtime.rs)), which builds every handler with
`sampling: None, elicitation: None, elicitation_complete: None`
([runtime.rs:1940-1953](../../crates/cyrup-mcp/src/runtime.rs)). The override
`ConnectionBuilder::with_handler_factory` ([runtime.rs:2296](../../crates/cyrup-mcp/src/runtime.rs)) and the manager's
`set_sampling_config` / `set_elicitation_config`
([server_manager.rs:1338](../../crates/cyrup-mcp/src/server_manager.rs), `:1343`) **all have zero callers.** So
`build_client_capabilities` ([runtime.rs:1220](../../crates/cyrup-mcp/src/runtime.rs)) always answers `{}`, and
`ClientHandler::create_message` ([runtime.rs:1545](../../crates/cyrup-mcp/src/runtime.rs)) always returns
`METHOD_NOT_FOUND`.

Upstream's shape is `createClient` reading `this.samplingConfig` / `this.elicitationConfig`
**per connection** (`server-manager.ts:691-741`). Reproduce that: the factory is a closure over a
`Weak<McpServerManager>` that re-reads the stored configs on every call.

### 2.2 `server_manager.rs` — store the whole config, and mint the factory

Replace the elicitation tuple ([server_manager.rs:1265](../../crates/cyrup-mcp/src/server_manager.rs)) with a named
struct so the completion notice has a route out — upstream reads `this.elicitationConfig.ui.notify`
off the same object (`server-manager.ts:734`), and splitting them is how the two drift.

```rust
// crates/cyrup-mcp/src/runtime.rs — beside `ElicitationHook` (runtime.rs:1395)

/// `options.ui.notify` — fire-and-forget in both implementations, which is why it returns `()` and
/// why a re-prompt dialog can open before its toast paints (same as upstream).
pub type NotifyHook = Arc<dyn Fn(&str, cyrup_ext::NotifyKind) + Send + Sync>;

/// `ServerElicitationConfig` (`elicitation-handler.ts:28`) — everything `createClient` needs to
/// build both elicitation hooks, minus the per-server name it splices in.
#[derive(Clone)]
pub struct ElicitationConfig {
    /// `allowUrl` — `ContextSnapshot::is_tui_mode`, stricter than `has_ui`.
    pub mode: ElicitationMode,
    /// `registerElicitationHandler`'s body.
    pub handler: ElicitationHook,
    /// The completion notice's only route out (MCP-469).
    pub notify: NotifyHook,
}
```

```rust
// crates/cyrup-mcp/src/server_manager.rs — replace the field at :1265 and the setter at :1343

    /// `elicitationConfig` — nulled by `close_all` so a late callback cannot re-enter a dead runtime.
    elicitation: Mutex<Option<crate::runtime::ElicitationConfig>>,

    /// `setElicitationConfig(config)` (`server-manager.ts:204-206`).
    pub fn set_elicitation_config(&self, elicitation: Option<crate::runtime::ElicitationConfig>) {
        *self.elicitation.lock().unwrap_or_else(PoisonError::into_inner) = elicitation;
    }
```

Then the factory itself, as a free function in `server_manager.rs` next to `ConnectionFactory`
([server_manager.rs:1148](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
/// `createClient(serverName, definition)`'s hook half (`server-manager.ts:691-741`).
///
/// # Why `Weak`
///
/// The manager owns the `ConnectionFactory`, the factory owns this closure, and this closure needs
/// the manager. An `Arc` here is a cycle that never drops and leaks every generation's connection
/// table. A dead weak yields the hookless handler, which is the correct answer rather than a
/// fallback: a manager that no longer exists advertises no capability it could service.
///
/// The configs are read **per call**, not captured, because upstream tests `this.samplingConfig` at
/// `createClient` time — `closeAll` nulls both (`server-manager.ts:1165-1166`) and a connect racing
/// a shutdown must see the null.
#[must_use]
pub fn manager_handler_factory(manager: Weak<McpServerManager>) -> crate::runtime::HandlerFactory {
    Arc::new(move |server: &str, runtime_signal: &CancelToken| {
        let Some(live) = manager.upgrade() else {
            return crate::runtime::bare_handler_factory()(server, runtime_signal);
        };
        let sampling = live.sampling.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let elicitation = live.elicitation.lock().unwrap_or_else(PoisonError::into_inner).clone();

        // `if (this.elicitationConfig.allowUrl) client.setNotificationHandler(...)` — the registration
        // gate. `McpClientHandler` applies the same `allow_url` test at dispatch
        // (runtime.rs:1622-1626), so passing the hook without `allow_url` is inert rather than wrong;
        // gating here as well keeps the two readings of "registered" identical.
        let complete = elicitation.as_ref().map(|config| {
            let back = Weak::clone(&manager);
            let notify = Arc::clone(&config.notify);
            Arc::new(move |event: crate::runtime::ElicitationCompleteEvent| {
                let Some(live) = back.upgrade() else { return };
                // `if (!accepted?.delete(id)) return;` — the notice fires ONLY on a delete that
                // removed something, so a duplicate completion is silent.
                if !live.forget_url_elicitation(&event.server, &event.elicitation_id) {
                    return;
                }
                notify(&url_elicitation_complete_notice(&event.server), cyrup_ext::NotifyKind::Info);
            }) as crate::runtime::ElicitationCompleteHook
        });

        crate::runtime::McpClientHandler::new(crate::runtime::McpClientHandlerParts {
            server: server.to_string(),
            runtime_signal: runtime_signal.clone(),
            elicitation_mode: elicitation.as_ref().map(|config| config.mode),
            sampling,
            elicitation: elicitation.as_ref().map(|config| Arc::clone(&config.handler)),
            list_changed: None, // MCP-120, not this task.
            elicitation_complete: complete,
        })
    })
}

/// `server-manager.ts:734-737` — the notice, verbatim.
#[must_use]
pub fn url_elicitation_complete_notice(server: &str) -> String {
    format!("MCP browser interaction for {server} completed. You can retry the tool now.")
}
```

### 2.3 `runtime.rs` — install it, and run `init.ts`'s two gates

`initialize_mcp` builds the manager at [runtime.rs:193-196](../../crates/cyrup-mcp/src/runtime.rs) and the state at
`:253-264`. The factory needs the manager and the manager needs the factory, so construct the
manager with `Arc::new_cyclic`. The hooks additionally need the generation's `McpDialog`, which lives
on `McpState` (`dialog()` at [state.rs:227](../../crates/cyrup-mcp/src/state.rs), because it reads
`human_wait_ctx` live) — so they close over a `Weak` slot filled the instant the state commits.

Replace [runtime.rs:193-196](../../crates/cyrup-mcp/src/runtime.rs) with:

```rust
    // The late-bound back-reference the sampling and elicitation hooks read the generation's dialog
    // through. Upstream's hooks close over `ui` directly (`init.ts:126-141`) because they are created
    // before `state`; here the dialog is `McpState::dialog()` — the ONE production constructor
    // (MCP-471) — because it also carries `human_wait_ctx`, which only the state has. `Weak`, never
    // `Arc`: the state owns the manager, the manager owns the hooks.
    let session: Arc<SessionSlot> = Arc::new(SessionSlot::default());

    let manager = Arc::new_cyclic(|weak: &std::sync::Weak<McpServerManager>| {
        let builder = ConnectionBuilder::new(Some(snapshot.cwd.clone()))
            .with_handler_factory(crate::server_manager::manager_handler_factory(weak.clone()));
        McpServerManager::with_factory(Some(snapshot.cwd.clone()), Arc::new(builder))
    });
```

and add, immediately after the four existing setters (`:205-215`):

```rust
    let settings = config.settings_or_default();

    // Step 5 — `init.ts:124-134`. `sampling !== false && (hasUI || samplingAutoApprove)`; the
    // predicate is already ported at config.rs:1227.
    if settings.sampling(snapshot.has_ui) {
        let options = Arc::new(crate::sampling::SamplingOptions {
            auto_approve: settings.sampling_auto_approve(),
            has_ui: snapshot.has_ui,
            session: Arc::clone(&session),
            models: Arc::clone(&models),
            owner: Arc::clone(&owner),
        });
        manager.set_sampling_config(Some(Arc::new(move |server, params| {
            let options = Arc::clone(&options);
            Box::pin(async move {
                crate::sampling::handle_sampling_request(&options, &server, params).await
            })
        })));
    }

    // Step 6 — `init.ts:135-141`. `elicitation !== false && hasUI`, `allowUrl = mode === "tui"`
    // (config.rs:1239 and ContextSnapshot::is_tui_mode, runtime.rs:86).
    if settings.elicitation(snapshot.has_ui)
        && let Some(ui) = ui.as_ref()
    {
        let options = Arc::new(crate::elicitation::ElicitationOptions {
            allow_url: snapshot.is_tui_mode(),
            session: Arc::clone(&session),
            launcher: Arc::new(crate::oauth::OpenerLauncher) as Arc<dyn crate::oauth::BrowserLauncher>,
        });
        let handler = {
            let options = Arc::clone(&options);
            Arc::new(move |server: String, params| {
                let options = Arc::clone(&options);
                Box::pin(async move {
                    crate::elicitation::handle_elicitation_request(&options, &server, params).await
                })
            }) as ElicitationHook
        };
        let notify = {
            // The FENCED handle: a stale generation's notice must not paint into the session that
            // replaced it. `OwnedServices::notify` degrades to `()` once the owner stops
            // (owner.rs:376).
            let ui = Arc::clone(ui);
            Arc::new(move |message: &str, kind| {
                cyrup_ext::HostServices::notify(ui.as_ref(), message, kind);
            }) as NotifyHook
        };
        manager.set_elicitation_config(Some(ElicitationConfig {
            mode: ElicitationMode { allow_url: snapshot.is_tui_mode() },
            handler,
            notify,
        }));
    }
```

and, immediately after `let state = Arc::new(McpState::new(...))` ([runtime.rs:253](../../crates/cyrup-mcp/src/runtime.rs)):

```rust
    session.bind(&state);
```

`SessionSlot` itself, beside `HandlerFactory` in `runtime.rs`:

```rust
/// The one-shot back-reference from a manager hook to the generation that created it.
///
/// `OnceLock` rather than a `Mutex`: it is written exactly once, by `initialize_mcp`, before any
/// connection can exist, and read from arbitrary rmcp tasks thereafter. A read before the write
/// yields `None`, which every consumer already has to handle — it is the same answer a headless
/// generation gives.
#[derive(Default)]
pub struct SessionSlot(std::sync::OnceLock<std::sync::Weak<McpState>>);

impl SessionSlot {
    /// Called once, by `initialize_mcp`, the moment the state commits.
    pub fn bind(&self, state: &Arc<McpState>) {
        let _ = self.0.set(Arc::downgrade(state));
    }

    /// The generation's dialog, or `None` for a headless or already-torn-down generation. `None` is
    /// upstream's `!state.ui`, and every consent gate must read it as "cannot ask", never "approved".
    #[must_use]
    pub fn dialog(&self) -> Option<crate::owner::McpDialog> {
        self.0.get()?.upgrade()?.dialog()
    }
}

impl std::fmt::Debug for SessionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSlot").field("bound", &self.0.get().is_some()).finish()
    }
}
```

`models` is the provider registry the sampling handler resolves against; see §3.3. Build it once,
above the manager, and share it by `Arc`.

**Units discharged:** `MCP-457` and `MCP-468` stop being inert; `MCP-469`'s producer lands; the call
sites `MCP-450`/`MCP-460` need exist.

---

## 3. `crates/cyrup-mcp/src/sampling.rs` — new module

Declare `pub mod sampling;` in [lib.rs](../../crates/cyrup-mcp/src/lib.rs) between `runtime` and `secrets`
(the list at `:132-151` is alphabetical). Open the module with the re-export the existing code
already promised ([owner.rs:53-61](../../crates/cyrup-mcp/src/owner.rs)):

```rust
//! `sampling-handler.ts` — `sampling/createMessage`.

// MCP-455 landed in `owner.rs` beside `McpDialog`, which it shares with MCP-232's tool-approval
// gate, because this module did not exist yet. Re-exported rather than moved: every path already
// written against `crate::owner::confirm_sampling` stays valid, which is the same technique
// `crate::state` uses for its forward declarations.
pub use crate::owner::{
    confirm_sampling, format_request_approval, format_response_approval, SamplingApproval,
    MESSAGE_TEXT_UNKNOWN_BLOCK, SAMPLING_REQUEST_APPROVAL_TITLE, SAMPLING_REQUEST_DECLINED,
    SAMPLING_REQUIRES_INTERACTIVE_APPROVAL, SAMPLING_RESPONSE_APPROVAL_TITLE,
};
```

> That is the complete list: the four `pub const`s at
> [owner.rs:604-618](../../crates/cyrup-mcp/src/owner.rs), `SamplingApproval` (`:629`),
> `confirm_sampling` (`:663`), the two formatters (`:696`, `:740`) and `MESSAGE_TEXT_UNKNOWN_BLOCK`
> (`:814`). **Nothing in `owner.rs` moves and nothing in it is rewritten.** One thing does change
> there: `sampling_block_type` (`:759`) becomes `pub(crate)` so §3.2's per-block guard reads the wire
> discriminant through the same function rather than growing a second copy.

### 3.1 The options bag (`MCP-458`)

Upstream's `SamplingHandlerOptions` (`sampling-handler.ts:18-25`) is six fields, two of which are
live accessors. Reproduce the accessors as **methods**, not captured values — that is the whole point
of `getCurrentModel` / `getSignal` being thunks.

```rust
/// `SamplingHandlerOptions` (`sampling-handler.ts:18-25`), minus `serverName`, which is per request.
pub struct SamplingOptions {
    /// `options.autoApprove` — `settings.samplingAutoApprove === true` (config.rs:1233).
    pub auto_approve: bool,
    /// `ctx.hasUI`, carried explicitly. See `SamplingApproval::has_ui` (owner.rs:637-655) for why
    /// this cannot be inferred from a `false` out of `HostServices::confirm`.
    pub has_ui: bool,
    /// `options.ui` — resolved LIVE, per dialog, through the generation's fenced handle.
    pub session: Arc<crate::runtime::SessionSlot>,
    /// `options.modelRegistry` — see §3.3 for why this is `cyrup-provider` and not `HostServices`.
    pub models: Arc<cyrup_provider::Models>,
    /// The generation owner. `owner.token()` is the `getSignal()` fallback for a stopped runtime.
    pub owner: Arc<crate::owner::McpRuntimeOwner>,
}

impl SamplingOptions {
    /// `getSignal: () => owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal) : owner.signal`
    /// (`init.ts:131-133`).
    ///
    /// **Mechanism divergence, recorded.** Upstream composes two `AbortSignal`s with
    /// `AbortSignal.any`; here a child `CancellationToken` is composed by `crate::abort::combine`
    /// (abort.rs:60) and passed down as `StreamOptions.cancel` rather than polled inside the
    /// completion. The observable behaviour — an in-flight sampling call dies when the turn is
    /// cancelled or the session reloads — is the same.
    ///
    /// **Called twice on purpose.** `handle_sampling_request` reads it once at entry and
    /// `resolve_sampling_model` reads it again inside the probe loop, exactly as upstream does
    /// (`sampling-handler.ts:40` and `:157`). A token captured once at entry diverges the moment the
    /// turn rolls over mid-request, which is precisely the case the second read exists for.
    #[must_use]
    pub fn signal(&self) -> CancelToken {
        if !self.owner.is_active() {
            return self.owner.token();
        }
        // `ctx.signal` is the host's per-run cancellation, polled through `is_run_cancelled()`.
        crate::abort::combine(&self.owner.token(), None)
    }

    /// `getCurrentModel: () => owner.isActive() ? ctx.model : undefined` (`init.ts:130`).
    #[must_use]
    pub fn current_model(&self) -> Option<String> {
        if !self.owner.is_active() {
            return None;
        }
        self.session.current_model()
    }

    /// `confirmSampling`'s three inputs, rebuilt per dialog so a generation that stopped between the
    /// request gate and the response gate is inert at the second one.
    fn approval(&self) -> SamplingApproval {
        SamplingApproval {
            auto_approve: self.auto_approve,
            has_ui: self.has_ui,
            dialog: self.session.dialog(),
        }
    }
}
```

Add `SessionSlot::current_model()` beside `SessionSlot::dialog()`: `self.0.get()?.upgrade()?.ui
.as_ref()?.current_model()` — `OwnedServices` already fences `current_model` at
[owner.rs:425](../../crates/cyrup-mcp/src/owner.rs), and going through the fenced handle is what makes a stopped
generation report `None` rather than the dead session's model.

### 3.2 The body (`MCP-450`, `MCP-451`, `MCP-456`)

Twelve steps, in `sampling-handler.ts:35-93`'s order. The five parameter guards **precede** the
content conversion, which **precedes** model resolution — a server asking for tools must never see a
dialog.

```rust
/// `handleSamplingRequest(options, request)` (`sampling-handler.ts:35-93`).
///
/// Returned as `Result<_, ErrorData>` because it is `ClientHandler::create_message`'s body: every
/// upstream `throw` here becomes a JSON-RPC `-32603`, which is what the TS SDK turns an uncaught
/// handler rejection into. The three `-32602` cases live in `elicitation.rs`, not here.
#[allow(deprecated)] // SEP-2577; the same suppression, for the same reason, as `crate::runtime`.
pub async fn handle_sampling_request(
    options: &SamplingOptions,
    server: &str,
    params: CreateMessageRequestParams,
) -> Result<CreateMessageResult, ErrorData> {
    let signal = options.signal();
    throw_if_aborted(&signal, None).map_err(internal)?;

    // Guards 1-5, in upstream's order, so the FIRST violated one is reported.
    // `params.task` (guard 0) is unrepresentable — see `SAMPLING_TASKS_UNSUPPORTED`.
    if params.include_context.is_some_and(|inclusion| inclusion != ContextInclusion::None) {
        return Err(internal_msg(SAMPLING_CONTEXT_UNSUPPORTED));
    }
    if params.tools.as_ref().is_some_and(|tools| !tools.is_empty()) {
        return Err(internal_msg(SAMPLING_TOOLS_UNSUPPORTED));
    }
    if params.tool_choice.is_some() {
        return Err(internal_msg(SAMPLING_TOOL_CHOICE_UNSUPPORTED));
    }
    if params.stop_sequences.as_ref().is_some_and(|stops| !stops.is_empty()) {
        return Err(internal_msg(SAMPLING_STOP_SEQUENCES_UNSUPPORTED));
    }

    // Guard 6 rides inside the conversion, exactly as upstream's does.
    let messages = params
        .messages
        .iter()
        .map(convert_sampling_message)
        .collect::<Result<Vec<Message>, ErrorData>>()?;

    let resolved = resolve_sampling_model(options, params.model_preferences.as_ref()).await?;
    throw_if_aborted(&signal, None).map_err(internal)?;

    confirm_sampling(
        &options.approval(),
        SAMPLING_REQUEST_APPROVAL_TITLE,
        &format_request_approval(
            server,
            &format!("{}/{}", resolved.provider.as_str(), resolved.id.as_str()),
            params.system_prompt.as_deref(),
            &messages,
        ),
    )
    .await
    .map_err(internal)?;
    throw_if_aborted(&signal, None).map_err(internal)?;

    let context = cyrup_provider::Context {
        system_prompt: params.system_prompt.clone(),
        messages,
        tools: Vec::new(),
    };
    let stream_options = cyrup_provider::StreamOptions {
        cancel: Some(signal.clone()),
        // `maxTokens: params.maxTokens` — passed through UNMODIFIED and UNCLAMPED. rmcp types it
        // `u32`; widening is lossless.
        max_tokens: Some(u64::from(params.max_tokens)),
        temperature: params.temperature,
        ..Default::default()
    };
    let assistant = options.models.complete(&resolved, &context, &stream_options).await;

    let converted = convert_assistant_result(&assistant)?;
    throw_if_aborted(&signal, None).map_err(internal)?;
    confirm_sampling(
        &options.approval(),
        SAMPLING_RESPONSE_APPROVAL_TITLE,
        &format_response_approval(server, &converted),
    )
    .await
    .map_err(internal)?;
    Ok(converted)
}
```

The six literals (`MCP-451`), all `pub const` so the taxonomy is greppable:

```rust
/// `sampling-handler.ts:44`. **No throw site, and this is structural, not an omission.**
/// `CreateMessageRequestParams` (`rmcp-3.1.4/src/model.rs:2865-2897`) has no `task` field: task
/// augmentation is the `io.modelcontextprotocol/tasks` extension, which this client never declares,
/// so a conforming server cannot send one and a non-conforming one has its `task` key dropped at
/// deserialisation. Written down so the day rmcp models it the arm is one `if` with the right text
/// already here — the same treatment `MESSAGE_TEXT_UNKNOWN_BLOCK` (owner.rs:814) gets.
pub const SAMPLING_TASKS_UNSUPPORTED: &str = "MCP sampling tasks are not supported";
/// `sampling-handler.ts:47`.
pub const SAMPLING_CONTEXT_UNSUPPORTED: &str = "MCP sampling context inclusion is not supported";
/// `sampling-handler.ts:50`.
pub const SAMPLING_TOOLS_UNSUPPORTED: &str = "MCP sampling tool use is not supported";
/// `sampling-handler.ts:53`.
pub const SAMPLING_TOOL_CHOICE_UNSUPPORTED: &str = "MCP sampling tool choice is not supported";
/// `sampling-handler.ts:56`.
pub const SAMPLING_STOP_SEQUENCES_UNSUPPORTED: &str = "MCP sampling stop sequences are not supported";
```

The conversion pair (`MCP-456`). The synthetic sentinels are **persisted-adjacent literals** — a
session that records a sampling round trip diverges from pi's bytes if they are "improved":

```rust
/// `convertSamplingMessage` (`sampling-handler.ts:196-216`).
///
/// `SamplingContent::{Single, Multiple}` (`rmcp-3.1.4/src/model.rs:2592-2595`) already models
/// upstream's `Array.isArray(content) ? content : [content]`, so the normalisation is `into_vec`.
#[allow(deprecated)]
fn convert_sampling_message(message: &SamplingMessage) -> Result<Message, ErrorData> {
    let blocks = message.content.clone().into_vec();
    let timestamp = now_millis();
    match message.role {
        Role::User => Ok(Message::User {
            content: blocks
                .iter()
                .map(|block| convert_text_block(block, USER_BLOCK_TEMPLATE))
                .collect::<Result<_, _>>()?,
            timestamp,
        }),
        // `api: "mcp-sampling"`, `provider: "mcp"`, `model: "sampling-request"`, `zeroUsage()`,
        // `stopReason: "stop"` — literal sentinels, not descriptions. Do not "improve" them.
        Role::Assistant => Ok(Message::Assistant(AssistantMessage {
            content: blocks
                .iter()
                .map(|block| convert_text_block(block, ASSISTANT_BLOCK_TEMPLATE))
                .collect::<Result<_, _>>()?,
            provider: ProviderId::from(SAMPLING_SYNTHETIC_PROVIDER),
            model: SAMPLING_SYNTHETIC_MODEL.to_string(),
            api: ApiId::from(SAMPLING_SYNTHETIC_API),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp,
        })),
    }
}

/// `sampling-handler.ts:222` / `:229` — the two per-role templates, which differ by one word.
const USER_BLOCK_TEMPLATE: &str = "MCP sampling {kind} content is not supported";
const ASSISTANT_BLOCK_TEMPLATE: &str = "MCP sampling assistant {kind} content is not supported";
/// `sampling-handler.ts:209-211`.
const SAMPLING_SYNTHETIC_API: &str = "mcp-sampling";
const SAMPLING_SYNTHETIC_PROVIDER: &str = "mcp";
const SAMPLING_SYNTHETIC_MODEL: &str = "sampling-request";
```

`{kind}` is the **wire discriminant**, read back off the serialised block — the identical technique
`sampling_block_type` already uses at [owner.rs:759-770](../../crates/cyrup-mcp/src/owner.rs), and for the identical
reason: `SamplingMessageContentBlock` is `#[non_exhaustive]`, so a hand-written match would have to
invent a name for a variant it has never seen. Reuse that function rather than writing a second one —
make it `pub(crate)` in `owner.rs` and call it.

```rust
/// `convertAssistantResult` (`sampling-handler.ts:232-260`) + `mapStopReason` (`:262-267`).
///
/// `CreateMessageResult::STOP_REASON_*` (`rmcp-3.1.4/src/model.rs:4150-4153`) are exactly
/// `mapStopReason`'s outputs, so the mapping is total with no string literals of our own.
fn convert_assistant_result(message: &AssistantMessage) -> Result<CreateMessageResult, ErrorData> {
    match message.stop_reason {
        StopReason::Error => {
            return Err(internal_msg(message.error_message.as_deref().unwrap_or(SAMPLING_CALL_FAILED)));
        }
        StopReason::Aborted => {
            return Err(internal_msg(message.error_message.as_deref().unwrap_or(SAMPLING_CALL_ABORTED)));
        }
        _ => {}
    }

    let mut parts: Vec<&str> = Vec::new();
    for block in &message.content {
        match block {
            Content::Text { text, .. } => parts.push(text),
            // `if (block.type === "thinking") return undefined` — dropped, not an error.
            Content::Thinking { .. } => {}
            Content::Image { .. } => return Err(internal_msg(&result_block_unsupported("image"))),
            Content::ToolCall(_) => return Err(internal_msg(&result_block_unsupported("toolCall"))),
        }
    }
    let text = parts.join("\n\n").trim().to_string();
    if text.is_empty() {
        return Err(internal_msg(SAMPLING_RESULT_EMPTY));
    }

    let stop_reason = match message.stop_reason {
        StopReason::Stop => CreateMessageResult::STOP_REASON_END_TURN,
        StopReason::Length => CreateMessageResult::STOP_REASON_END_MAX_TOKEN,
        StopReason::ToolUse => CreateMessageResult::STOP_REASON_TOOL_USE,
        // `return reason` — every other spelling passes through verbatim.
        other => other.as_wire_str(),
    };
    Ok(CreateMessageResult::new(
        SamplingMessage {
            role: Role::Assistant,
            content: SamplingContent::Single(SamplingMessageContentBlock::Text(TextContent::new(text))),
            meta: None,
        },
        format!("{}/{}", message.provider.as_str(), message.model),
    )
    .with_stop_reason(stop_reason))
}
```

### 3.3 Candidate ordering and the auth probe (`MCP-452`, `MCP-453`, `MCP-454`)

`HostServices::models()` is **not** the source. It is a bare `Value` whose only implementations are
the trait default `json!([])` ([host/services.rs:448](../../crates/cyrup-ext/src/host/services.rs)) and a test
recorder at `:1001` — no live host implements it. `cyrup-provider` is the literal upstream mechanism
(`sampling-handler.ts:1` imports `complete` from the AI package directly, bypassing the host API),
and the manifest edge already exists for exactly this reason
([cyrup-mcp/Cargo.toml:45-50](../../crates/cyrup-mcp/Cargo.toml)).

Build the registry once, in `initialize_mcp`, above the manager:

```rust
    // `ctx.modelRegistry` — `Models::get_available` (collection.rs:584) is `getAvailable()` and
    // `Models::get_auth` (collection.rs:216) is `getApiKeyAndHeaders`. `default_models` spans EVERY
    // built-in provider, which is what `getAvailable()` spans; one installed provider's catalogue
    // would be narrower and is the bug the spec names at 13i:930.
    let models = Arc::new(cyrup_provider::default_models(cyrup_provider::CreateModelsOptions::default()));
```

The pure orderer — hint-order-major, registry-order-minor, first-wins dedupe:

```rust
/// `resolveSamplingModel`'s candidate assembly (`sampling-handler.ts:135-154`) with
/// `addSamplingCandidate` (`:179-183`) inlined as the dedupe.
///
/// Order is behaviour, not taste: hints in the server's order (each hint scanning the whole
/// registry in registry order), then the session's current model, then everything else.
/// `Model { id, name, provider }` (`cyrup-provider/src/model.rs:54-58`) gives exactly upstream's
/// three searchable names.
#[must_use]
pub fn sampling_candidates(available: &[Model], hints: &[String], current: Option<&Model>) -> Vec<Model> {
    let mut candidates: Vec<Model> = Vec::new();
    let mut push = |candidates: &mut Vec<Model>, model: &Model| {
        if !candidates
            .iter()
            .any(|seen| seen.provider == model.provider && seen.id == model.id)
        {
            candidates.push(model.clone());
        }
    };

    for hint in hints {
        let needle = hint.trim().to_lowercase();
        // `if (!normalizedHint) continue;` — an empty or whitespace-only hint matches nothing rather
        // than everything, which is what a bare `.contains("")` would do.
        if needle.is_empty() {
            continue;
        }
        for model in available {
            let haystacks = [
                format!("{}/{}", model.provider.as_str(), model.id.as_str()),
                model.id.as_str().to_string(),
                model.name.clone(),
            ];
            // Plain lowercase substring. NOT fuzzy matching.
            if haystacks.iter().any(|name| name.to_lowercase().contains(&needle)) {
                push(&mut candidates, model);
            }
        }
    }
    if let Some(current) = current {
        push(&mut candidates, current);
    }
    for model in available {
        push(&mut candidates, model);
    }
    candidates
}
```

The probe loop, with the second signal read (`sampling-handler.ts:156-177`):

```rust
async fn resolve_sampling_model(
    options: &SamplingOptions,
    preferences: Option<&ModelPreferences>,
) -> Result<Model, ErrorData> {
    let available = options.models.get_available(None).await;
    let hints: Vec<String> = preferences
        .and_then(|preferences| preferences.hints.as_ref())
        .map(|hints| hints.iter().filter_map(|hint| hint.name.clone()).collect())
        .unwrap_or_default();
    let current = options
        .current_model()
        .and_then(|id| available.iter().find(|model| model.id.as_str() == id).cloned());
    let candidates = sampling_candidates(&available, &hints, current.as_ref());

    let mut errors: Vec<String> = Vec::new();
    // `const signal = options.getSignal();` — the SECOND read (`sampling-handler.ts:157`).
    let signal = options.signal();
    for model in candidates {
        throw_if_aborted(&signal, None).map_err(internal)?;
        let auth = options.models.get_auth(&model).await;
        throw_if_aborted(&signal, None).map_err(internal)?;
        match auth {
            // `auth.ok === false` — recorded and SKIPPED, never fatal on its own.
            Err(error) => errors.push(format!(
                "{}/{}: {error}",
                model.provider.as_str(),
                model.id.as_str()
            )),
            Ok(None) => errors.push(format!(
                "{}/{}: {NO_CONFIGURED_AUTH_DETAIL}",
                model.provider.as_str(),
                model.id.as_str()
            )),
            Ok(Some(_)) => return Ok(model),
        }
    }

    // The two exhaustion messages, and which one you get is observable.
    if errors.is_empty() {
        return Err(internal_msg(NO_MODEL_AVAILABLE));
    }
    Err(internal_msg(&format!("{NO_CONFIGURED_AUTH}. {}", errors.join("; "))))
}

/// `sampling-handler.ts:174`.
pub const NO_CONFIGURED_AUTH: &str = "No configured auth for MCP sampling model";
/// `sampling-handler.ts:176` — cyrup renames pi to cyrup, as every user-facing string in this port does.
pub const NO_MODEL_AVAILABLE: &str = "No cyrup model is available for MCP sampling";
```

`apiKey`/`headers` are **not** threaded into `StreamOptions`. `Models::complete` re-resolves auth
itself through the same `resolve_provider_auth`
([collection.rs:284-297](../../crates/cyrup-provider/src/collection.rs)), so `get_auth` here is the *probe*
— which is all `getApiKeyAndHeaders` is used for on the candidate loop — and passing the key twice
would be the only way to make the two disagree. Record that as the one deliberate divergence in this
module's doc.

`params.metadata` (`sampling-handler.ts:80`) has **no** `StreamOptions` counterpart. Drop it and say
so in a comment naming the field; `StreamOptions::sampling_params` is a different thing (provider
sampling knobs, not MCP request metadata) and using it would send a server's opaque bag to the
provider as request parameters.

---

## 4. `crates/cyrup-mcp/src/schema.rs` — one validator, two consumers

`MCP-465` (elicitation's final assertion) and `MCP-092` (13b's `outputSchema` gate) need the same
compiled-and-cached, `$schema`-dispatching validator. Build it once. Two validators in one crate that
disagree about whether `format` is an assertion is the failure this module exists to prevent, and the
manifest already writes the contract down at
[cyrup-mcp/Cargo.toml:125-131](../../crates/cyrup-mcp/Cargo.toml).

```rust
//! `json-schema-validator.ts` — the dual-dialect gate.
//!
//! rmcp validates NOTHING client-side (there is no `jsonSchemaValidator` option on
//! `Peer<RoleClient>`, unlike the TS SDK's), so this is not optional and not a duplicate.

/// `DRAFT_07_SCHEMA_URIS` (`json-schema-validator.ts:18-21`).
const DRAFT_07_URIS: [&str; 2] =
    ["http://json-schema.org/draft-07/schema", "https://json-schema.org/draft-07/schema"];
/// `DRAFT_2020_12_SCHEMA_URIS` (`json-schema-validator.ts:22-24`).
const DRAFT_2020_12_URIS: [&str; 1] = ["https://json-schema.org/draft/2020-12/schema"];

/// `schemaDialect(schema)` (`json-schema-validator.ts:26-34`).
///
/// A NON-STRING `$schema` is `unstamped`, not an error — `typeof schema.$schema !== "string"`. And
/// exactly ONE trailing `#` is stripped, so `…/schema##` stays unrecognised.
fn dialect(schema: &Value) -> Option<&str> {
    let raw = schema.get("$schema")?.as_str()?;
    Some(raw.strip_suffix('#').unwrap_or(raw))
}

/// `Unsupported JSON Schema dialect: ${uri}` (`json-schema-validator.ts:53`).
#[must_use]
pub fn unsupported_dialect_message(uri: &str) -> String {
    format!("Unsupported JSON Schema dialect: {uri}")
}

/// `createJsonSchemaValidator().getValidator(schema)` — compile, or say which dialect was refused.
///
/// `should_validate_formats(true)` is the whole point: `jsonschema` treats `format` as an
/// ANNOTATION by default, which would silently disable `format: "email"` — the one constraint the
/// elicitation coercion pass cannot express. `default-features = false` keeps remote/file `$ref`
/// resolution off, per the workspace pin at `Cargo.toml:189`.
///
/// # Errors
///
/// [`unsupported_dialect_message`] for a stamped-but-unknown dialect; the compiler's own message for
/// a schema that will not build.
pub fn compile(schema: &Value) -> McpResult<jsonschema::Validator> {
    match dialect(schema) {
        // Unstamped and 2020-12 both take the 2020-12 arm — upstream's `??=` order.
        None => {}
        Some(uri) if DRAFT_2020_12_URIS.contains(&uri) => {}
        Some(uri) if DRAFT_07_URIS.contains(&uri) => {}
        Some(uri) => return Err(McpError::Config(unsupported_dialect_message(uri))),
    }
    jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| McpError::Config(error.to_string()))
}

/// The per-schema compile cache. `coerceAndValidateFormValues` runs once per field AND once per
/// review pass over the same `requestedSchema`, so an uncached compile is O(fields²).
///
/// Keyed on `stable_stringify` (dirs.rs) rather than on a pointer: the single-property synthetic
/// schemas `collect_valid_field` builds are fresh values every iteration, and only a content key
/// dedupes them.
#[derive(Default)]
pub struct ValidatorCache {
    entries: Mutex<HashMap<String, Arc<jsonschema::Validator>>>,
}
```

Give `ValidatorCache` one method, `get_or_compile(&self, schema: &Value) -> McpResult<Arc<Validator>>`,
recovering a poisoned lock with `PoisonError::into_inner` — the same policy
[server_manager.rs:2331](../../crates/cyrup-mcp/src/server_manager.rs) uses, and for the same reason: `unwrap_used`
is denied crate-wide and a half-written cache is not representable.

Declare `pub mod schema;` in `lib.rs`.

---

## 5. `crates/cyrup-mcp/src/elicitation.rs` — new module

Declare `pub mod elicitation;` in `lib.rs`. `MCP-460`'s dispatch is one `match`; rmcp's
`ElicitRequestParamsWire::LegacyForm` untagged arm (`rmcp-3.1.4/src/model.rs:3599-3606`) already gives
upstream's *absent-or-unknown `mode` → form* for free.

### 5.0 The module's three types

```rust
/// `ElicitationValue` (`elicitation-handler.ts:15`) — `string | number | boolean | string[] |
/// undefined`. `None` is `undefined`, which is a distinct, meaningful state: "omitted", which the
/// coercion pass turns into either a skip or the missing-required throw.
pub type ElicitationValue = Option<FieldValue>;

/// The four inhabited spellings. A closed enum rather than `serde_json::Value` so the coercion
/// pass's `match` is exhaustive and `js_bool`/`js_number` cannot be handed an object.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Bool(bool),
    List(Vec<String>),
}

/// `FieldCollectionResult` (`elicitation-handler.ts:17`).
enum FieldOutcome {
    Cancelled,
    Collected(ElicitationValue),
}

/// `ElicitationHandlerOptions` (`elicitation-handler.ts:21-26`), minus `serverName`.
pub struct ElicitationOptions {
    /// `options.allowUrl` — `mode === "tui"`, NOT `hasUI`.
    pub allow_url: bool,
    /// The generation's fenced dialog source (§2.3).
    pub session: Arc<crate::runtime::SessionSlot>,
    /// `import open from "open"` — see C-2. `OpenerLauncher` in production, `NoopLauncher` headless.
    pub launcher: Arc<dyn crate::oauth::BrowserLauncher>,
    /// `options.onUrlAccepted` — the manager's `remember_url_elicitation` (server_manager.rs:2582).
    pub on_url_accepted: Arc<dyn Fn(&str) + Send + Sync>,
    /// The shared compile cache of §4. The validator runs once per field AND once per review pass.
    pub validators: Arc<crate::schema::ValidatorCache>,
}
```

The five fixed dialog labels, as `pub const`s, because `HostServices::select` returns the chosen
**string** and every comparison in this module is against one of them:
`CONTINUE = "Continue"`, `DECLINE = "Decline"`, `SUBMIT = "Submit"`, `EDIT = "Edit"`,
`OPEN = "Open"`, plus `CHOOSE_A_FIELD = "Choose a field to edit"`
(`elicitation-handler.ts:51`, `:68`, `:75`, `:331`).

### 5.1 `McpDialog` grows two arms first (`MCP-471`)

Both are additive to [owner.rs:575-597](../../crates/cyrup-mcp/src/owner.rs) and both must land **before** the
field loop is written, or the loop grows its own dialog path and the one-serialized-route invariant
is silently broken. `HostServices::input` and `::notify` already exist
([host/services.rs:200](../../crates/cyrup-ext/src/host/services.rs), `:304`) and are already fenced on
`OwnedServices` ([owner.rs:376](../../crates/cyrup-mcp/src/owner.rs), `:391`).

```rust
    /// `ui.input(title, placeholder)` — the typed value, or `None` for a dismissal.
    ///
    /// `placeholder` is upstream's seed: `current === undefined ? undefined : String(current)`, so a
    /// re-prompt after a validation failure does not lose what the user typed.
    pub async fn input(&self, prompt: &str, placeholder: Option<&str>) -> Option<String> {
        let _guards = self.enter().await;
        self.services.input(prompt, placeholder, &cyrup_ext::DialogOptions::default())
    }

    /// `ui.notify(message, kind)` — fire-and-forget, and deliberately NOT under `enter()`.
    ///
    /// A toast asks the human nothing, so taking the interaction lock for it would make a validation
    /// message queue behind the very dialog it is about to be shown beside. Upstream's `notify` is
    /// likewise outside every `await ui.select(...)`.
    pub fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) {
        self.services.notify(message, kind);
    }
```

### 5.2 The form leg (`MCP-461`, `MCP-462`, `MCP-463`)

The **one** ordered read that drives four user-visible orderings — questions, review rows, edit-picker
labels, coercion order:

```rust
/// `Object.entries(params.requestedSchema.properties)` — the ONE ordered read
/// (`elicitation-handler.ts:48`, `:198`).
///
/// `ElicitationSchema::properties` is a `BTreeMap` (`rmcp-3.1.4/src/model/elicitation_schema.rs:1136`),
/// which is LEXICOGRAPHIC — iterating it directly is the silent bug MCP-462 names. `property_order`
/// (`:1141`) is the wire order, filled from the `IndexMap` the wire type deserialises through
/// (`:1169-1181`); it is `None` only for a schema this process constructed itself, where the
/// BTreeMap order is the only order there ever was.
fn ordered_properties(schema: &ElicitationSchema) -> Vec<(&str, &PrimitiveSchemaDefinition)> {
    match schema.property_order.as_ref() {
        Some(order) => order
            .iter()
            .filter_map(|name| schema.properties.get_key_value(name.as_str()))
            .map(|(name, definition)| (name.as_str(), definition))
            .collect(),
        None => schema
            .properties
            .iter()
            .map(|(name, definition)| (name.as_str(), definition))
            .collect(),
    }
}
```

Then `handle_form_elicitation`, transcribing `elicitation-handler.ts:44-84`. **Two behaviours a Rust
engineer will want to "fix" and must not:**

* the review loop's `coerce_and_validate` call is **not** caught — a cross-field failure escapes as a
  JSON-RPC error, not as an `ElicitResult`;
* duplicate edit labels resolve **first-wins** via `indexOf` (`:77`), which in Rust is
  `labels.iter().position(...)`, not a `HashMap`.

```rust
pub async fn handle_form_elicitation(
    options: &ElicitationOptions,
    server: &str,
    message: &str,
    schema: &ElicitationSchema,
) -> Result<ElicitResult, ErrorData> {
    let Some(dialog) = options.session.dialog() else {
        // No UI to ask through. rmcp's own default for an unwired handler is Decline
        // (runtime.rs:1573), and a client that cannot ask must not accept on the user's behalf.
        return Ok(ElicitResult::new(ElicitationAction::Decline));
    };
    let properties = ordered_properties(schema);

    let gate = format!("MCP Input Request\nServer: {server}\n\n{message}");
    match dialog.select(&gate, &[CONTINUE, DECLINE]).await.as_deref() {
        None => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
        Some(DECLINE) => return Ok(ElicitResult::new(ElicitationAction::Decline)),
        Some(_) => {}
    }
    // `if (properties.length === 0) return { action: "accept", content: {} }` — BEFORE any review
    // screen, and it is an empty object, not `None`.
    if properties.is_empty() {
        return Ok(ElicitResult::new(ElicitationAction::Accept).with_content(json!({})));
    }

    let mut values: IndexMap<String, ElicitationValue> = IndexMap::new();
    for (name, definition) in &properties {
        match collect_valid_field(options, &dialog, schema, name, definition, None).await? {
            FieldOutcome::Cancelled => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
            FieldOutcome::Collected(value) => {
                values.insert((*name).to_string(), value);
            }
        }
    }

    loop {
        // NOT caught. A cross-field failure here is a JSON-RPC error, exactly as upstream's is.
        let content = coerce_and_validate(options, schema, &values)?;
        let review = format_review(server, &properties, &content);
        let action = dialog.select(&review, &[SUBMIT, EDIT, DECLINE]).await;
        match action.as_deref() {
            None => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
            Some(DECLINE) => return Ok(ElicitResult::new(ElicitationAction::Decline)),
            Some(SUBMIT) => {
                return Ok(ElicitResult::new(ElicitationAction::Accept).with_content(content))
            }
            Some(_) => {}
        }

        // The edit picker labels are deliberately NOT uniquified upstream (`:74`), and the lookup is
        // `indexOf`, i.e. first wins. Both are load-bearing: two identically-titled fields make the
        // second unreachable from the picker, and that is upstream's behaviour.
        let labels: Vec<String> = properties
            .iter()
            .map(|(name, definition)| {
                format!("{} ({name})", title_of(definition).unwrap_or_else(|| humanize_name(name)))
            })
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let Some(selected) = dialog.select(CHOOSE_A_FIELD, &refs).await else {
            return Ok(ElicitResult::new(ElicitationAction::Cancel));
        };
        // `if (!property) continue;` — a selection that matches no label re-runs the review loop.
        let Some(index) = labels.iter().position(|label| *label == selected) else { continue };
        let Some((name, definition)) = properties.get(index) else { continue };
        let current = values.get(*name).cloned().flatten();
        match collect_valid_field(options, &dialog, schema, name, definition, current).await? {
            FieldOutcome::Cancelled => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
            FieldOutcome::Collected(value) => {
                values.insert((*name).to_string(), value);
            }
        }
    }
}
```

`collect_valid_field` (`elicitation-handler.ts:86-112`) is the unbounded re-prompt loop. Its synthetic
schema **copies the whole schema and replaces only `properties`/`required`**, so a sibling constraint
that lives on the parent survives:

```rust
/// `collectValidField` — validate one field against a single-property schema, notify, re-ask.
///
/// The `#[must_use]` human-wait guard belongs to the LOOP, not to each dialog: an unbounded
/// re-prompt is exactly the case P-3 forgiveness exists for. `McpDialog::enter` already takes it per
/// call, which is strictly safer; what matters is that no arm of this loop bypasses `McpDialog`.
async fn collect_valid_field(
    options: &ElicitationOptions,
    dialog: &McpDialog,
    schema: &ElicitationSchema,
    name: &str,
    definition: &PrimitiveSchemaDefinition,
    mut current: ElicitationValue,
) -> Result<FieldOutcome, ErrorData> {
    let required = schema.required.as_ref().is_some_and(|names| names.iter().any(|n| n == name));
    let mut single = schema.clone();
    single.properties = std::iter::once((name.to_string(), definition.clone())).collect();
    single.property_order = Some(vec![name.to_string()]);
    single.required = required.then(|| vec![name.to_string()]);

    loop {
        let outcome = collect_field(dialog, schema, name, definition, current.clone()).await;
        let FieldOutcome::Collected(value) = outcome else { return Ok(FieldOutcome::Cancelled) };
        let mut one = IndexMap::new();
        one.insert(name.to_string(), value.clone());
        match coerce_and_validate(options, &single, &one) {
            Ok(_) => return Ok(FieldOutcome::Collected(value)),
            Err(error) => {
                dialog.notify(&error.message, cyrup_ext::NotifyKind::Error);
                current = value;
            }
        }
    }
}
```

`collect_field` (`elicitation-handler.ts:114-190`) is a `match` on `PrimitiveSchemaDefinition`
(`rmcp-3.1.4/src/model/elicitation_schema.rs:54-65`) — rmcp's closed enum turns upstream's schema
sniffing into arms:

| upstream test | rmcp arm |
|---|---|
| `type==="string" && "oneOf" in schema` | `Enum(EnumSchema::Single(SingleSelectEnumSchema::Titled(_)))` — `one_of: Vec<ConstTitle>` |
| `type==="string" && "enum" in schema` | `Enum(EnumSchema::Single(SingleSelectEnumSchema::Untitled(_)))` — `enum_: Vec<String>` |
| `enumNames` | `Enum(EnumSchema::Legacy(_))` — `enum_` + `enum_names` |
| `type==="array"` | `Enum(EnumSchema::Multi(MultiSelectEnumSchema::{Untitled,Titled}))` |
| `type==="boolean"` | `Boolean(BooleanSchema)` |
| everything else | `String(_)` / `Number(_)` / `Integer(_)` — the `input` arm |

**The third named hazard is real and must be recorded, not worked around:** upstream's "any other
`type`" arm silently *drops* the field; rmcp's closed untagged enum instead fails deserialisation of
the whole `elicitation/create` request. Write that delta in the module doc.

The `MCP-466` helpers, all pure:

```rust
/// `formatChoice(value, title)` (`elicitation-handler.ts:268-270`) — a title equal to the value is
/// suppressed, not duplicated.
#[must_use]
pub fn format_choice(value: &str, title: Option<&str>) -> String {
    match title {
        Some(title) if title != value => format!("{title} ({value})"),
        _ => value.to_string(),
    }
}

/// `uniqueLabels` (`:272-280`) — append U+2026 until unique, against an ACCUMULATING set.
///
/// Necessary because `HostServices::select` returns the chosen STRING, not an index: two identical
/// labels would make the second unselectable. The edit picker deliberately does NOT use this.
#[must_use]
pub fn unique_labels(labels: &[String]) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    labels
        .iter()
        .map(|label| {
            let mut unique = label.clone();
            while !used.insert(unique.clone()) {
                unique.push('…');
            }
            unique
        })
        .collect()
}

/// `uniqueAction(label, choices)` (`:282-286`) — the same trick for an action added BESIDE the
/// choices, tested against the list rather than a set.
#[must_use]
pub fn unique_action(label: &str, choices: &[String]) -> String {
    let mut unique = label.to_string();
    while choices.iter().any(|choice| *choice == unique) {
        unique.push('…');
    }
    unique
}

/// `humanizeName(name)` (`:346-348`) — three replacements, in order: `[_-]+` → space,
/// lowerUpper → split, then upper-case the first character.
#[must_use]
pub fn humanize_name(name: &str) -> String { /* three `LazyLock<Regex>`, see MCP-474 for the pattern */ }
```

### 5.3 Coercion (`MCP-464`) — 13 templates, and JS `Number()`

Read the **typed** limit fields, never JSON. Three hazards, all named in the spec at `13i:1088-1096`
and all verified against the rmcp types:

```rust
/// `Number(value)` — JavaScript's, not `str::parse::<f64>()`.
///
/// Node-verified divergences that matter here: `"0x1f"` → 31, `"1e3"` → 1000, `"Infinity"` → ∞,
/// `" 7 "` → 7 (surrounding whitespace trimmed), `""` → 0, `"7abc"` → NaN. `str::parse` rejects the
/// first two and the fourth, and errors rather than yielding 0 on the fifth — so a blank optional
/// numeric field would take a different branch.
fn js_number(value: &str) -> f64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).map_or(f64::NAN, |value| value as f64);
    }
    // `0o`/`0b` take the same shape; `Infinity`/`-Infinity` are spelled out.
    match trimmed {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        other => other.parse::<f64>().unwrap_or(f64::NAN),
    }
}

/// `output[name] = typeof value === "boolean" ? value : value === "true"` (`:241`).
/// Every OTHER string is `false`, silently. Do not substitute `bool::from_str`, which errors.
fn js_bool(value: &ElicitationValue) -> bool { /* … */ }
```

The 13 templates are `elicitation-handler.ts:201`, `:208`, `:211`, `:214`, `:217`, `:224`, `:227`,
`:229`, `:232`, `:235`, `:245`, `:249`, `:252`, `:255`. Write each as a `pub fn …_message(…) ->
String` so the taxonomy is greppable and the re-prompt loop's `notify` text is the same string the
JSON-RPC error carries. Close with the schema assertion:

```rust
    // `new AjvJsonSchemaValidator().getValidator(requestedSchema)(output)` (`:260-264`).
    let validator = options.validators.get_or_compile(&serde_json::to_value(schema)?)?;
    if let Err(error) = validator.validate(&output) {
        return Err(invalid_elicitation_response(&error.to_string()));
    }
```

```rust
/// `` `Invalid elicitation response: ${errorMessage}` `` (`elicitation-handler.ts:263`).
///
/// The MESSAGE differs from ajv's — `jsonschema`'s renderer is its own — and that is accepted. The
/// PREFIX is load-bearing and must be byte-exact.
#[must_use]
pub fn invalid_elicitation_response(detail: &str) -> String {
    format!("Invalid elicitation response: {detail}")
}
```

### 5.4 The URL leg (`MCP-467`, `MCP-472`)

Three `-32602`s, and every other throw in both handlers stays `-32603`. Per **C-2**, the launcher
question does not exist: upstream calls the bare `open` package, which is `opener::open`, which is
[oauth.rs:2396](../../crates/cyrup-mcp/src/oauth.rs) `OpenerLauncher` behind the existing
`BrowserLauncher` trait (`:2382`). Take it by `Arc<dyn BrowserLauncher>` on `ElicitationOptions` so a
headless embedding can install `NoopLauncher` (`:2404`).

```rust
/// `handleUrlElicitation` (`elicitation-handler.ts:305-344`).
pub async fn handle_url_elicitation(
    options: &ElicitationOptions,
    server: &str,
    message: &str,
    url: &str,
    elicitation_id: &str,
) -> Result<ElicitResult, ErrorData> {
    if !options.allow_url {
        return Err(ErrorData::invalid_params(URL_ELICITATION_UNSUPPORTED, None));
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return Err(ErrorData::invalid_params(URL_ELICITATION_INVALID_URL, None));
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ErrorData::invalid_params(URL_ELICITATION_SCHEME, None));
    }
    let Some(dialog) = options.session.dialog() else {
        return Ok(ElicitResult::new(ElicitationAction::Decline));
    };

    // The nine lines, joined with "\n". `Host:` is host+port — `Url::host_str` drops the port, and
    // `URL.host` in JS keeps it, so a non-default port must be re-appended or the user is shown a
    // different address than the one that will open. `Full URL:` is the RAW input, never
    // `parsed.as_str()`: `Url::parse` normalises (trailing slash, percent-encoding, case), and the
    // point of the line is to show exactly what the server asked for.
    let host = match parsed.port() {
        Some(port) => format!("{}:{port}", parsed.host_str().unwrap_or_default()),
        None => parsed.host_str().unwrap_or_default().to_string(),
    };
    let prompt = [
        "MCP Browser Request",
        &format!("Server: {server}"),
        "",
        message,
        "",
        &format!("Host: {host}"),
        &format!("Full URL: {url}"),
        "",
        "Open this URL in your browser?",
    ]
    .join("\n");

    match dialog.select(&prompt, &[OPEN, DECLINE]).await.as_deref() {
        None => return Ok(ElicitResult::new(ElicitationAction::Cancel)),
        Some(DECLINE) => return Ok(ElicitResult::new(ElicitationAction::Decline)),
        Some(_) => {}
    }

    // `opener::open` is BLOCKING. Off the worker, unlike the dialogs above — `HostServices::confirm`
    // does its own `block_in_place` internally (owner.rs:516-521) and this does not.
    let launcher = Arc::clone(&options.launcher);
    let target = url.to_string();
    let opened = tokio::task::spawn_blocking(move || launcher.open(&target))
        .await
        .map_err(|_| internal_msg(URL_ELICITATION_OPEN_FAILED))?;
    if let Err(error) = opened {
        dialog.notify(&could_not_open_message(&error.to_string()), cyrup_ext::NotifyKind::Error);
        // CANCEL, not decline: the user said yes and the machine failed.
        return Ok(ElicitResult::new(ElicitationAction::Cancel));
    }

    // `options.onUrlAccepted?.(params.elicitationId)` — the registry write MCP-469's dedupe reads.
    (options.on_url_accepted)(elicitation_id);
    dialog.notify(OPENED_BROWSER_NOTICE, cyrup_ext::NotifyKind::Info);
    Ok(ElicitResult::new(ElicitationAction::Accept))
}

/// `elicitation-handler.ts:309`.
pub const URL_ELICITATION_UNSUPPORTED: &str = "URL elicitation is not supported";
/// `:315`.
pub const URL_ELICITATION_INVALID_URL: &str = "URL elicitation supplied an invalid URL";
/// `:318`.
pub const URL_ELICITATION_SCHEME: &str = "URL elicitation only supports HTTP and HTTPS URLs";
/// `:342`.
pub const OPENED_BROWSER_NOTICE: &str = "Opened browser for MCP elicitation.";
```

`on_url_accepted` is `Arc<dyn Fn(&str) + Send + Sync>`, supplied by `manager_handler_factory` as
`|id| live.remember_url_elicitation(server, id)` — the registry at
[server_manager.rs:2582](../../crates/cyrup-mcp/src/server_manager.rs), whose aborted-runtime no-op is already
implemented.

### 5.5 `handleUrlElicitationRequired` (`MCP-470`) — the caller-side half

The caller-side arm is already ported: `ProxyEnv::handle_url_elicitation_required`
([proxy/env.rs:295](../../crates/cyrup-mcp/src/proxy/env.rs)) and the three action messages
([proxy/call.rs:860-871](../../crates/cyrup-mcp/src/proxy/call.rs)). What is missing is the manager method
they call, and the `-32042` decode. Add beside `remember_url_elicitation`:

```rust
    /// `handleUrlElicitationRequired(serverName, error)` (`server-manager.ts:800-814`).
    ///
    /// Sequential and short-circuiting: the FIRST non-accept answer ends the loop and is returned,
    /// so a decline on elicitation 2 of 3 never opens elicitation 3.
    pub async fn handle_url_elicitation_required(
        self: &Arc<Self>,
        server: &str,
        error: &ErrorData,
    ) -> UrlElicitationAction {
        let aborted = self
            .runtime_signal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(CancelToken::is_cancelled);
        let config = self.elicitation.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let Some(config) = config.filter(|config| config.mode.allow_url) else {
            return UrlElicitationAction::Cancel;
        };
        if aborted {
            return UrlElicitationAction::Cancel;
        }
        for params in decode_url_elicitations(error) {
            match (config.handler)(server.to_string(), params).await {
                Ok(result) => match result.action {
                    ElicitationAction::Accept => {}
                    ElicitationAction::Decline => return UrlElicitationAction::Decline,
                    _ => return UrlElicitationAction::Cancel,
                },
                Err(_) => return UrlElicitationAction::Cancel,
            }
        }
        UrlElicitationAction::Accept
    }

/// `UrlElicitationRequiredError`'s payload: `ProtocolErrorCode.UrlElicitationRequired` = **-32042**
/// with `data.elicitations` (`@modelcontextprotocol/client` `src-NAgB4Mp8.cjs:3458-3460`, `:3512-3521`).
///
/// rmcp models neither the code nor the error class, so the decode is by hand. A payload that does
/// not parse yields an EMPTY list, which makes `handle_url_elicitation_required` return `Accept`
/// having opened nothing — the same answer upstream's empty `error.elicitations` gives.
const URL_ELICITATION_REQUIRED_CODE: i32 = -32042;

fn decode_url_elicitations(error: &ErrorData) -> Vec<ElicitRequestParams> {
    if error.code.0 != URL_ELICITATION_REQUIRED_CODE {
        return Vec::new();
    }
    error
        .data
        .as_ref()
        .and_then(|data| data.get("elicitations"))
        .and_then(|list| serde_json::from_value::<Vec<ElicitRequestParams>>(list.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|params| matches!(params, ElicitRequestParams::UrlElicitationParams { .. }))
        .collect()
}
```

---

## 6. `crates/cyrup-mcp/src/trace.rs` — new module

Declare `pub mod trace;`. Independent of §2-§5: it decorates the transport, not the handler, and can
land first.

### 6.1 The event (`MCP-473`, `MCP-475`)

Serde emits struct fields in **declaration order**, so declare them in `createMcpTraceEvent`'s
**insertion** order (`mcp-trace.ts:99-119`), **not** the interface order at `:26-40` — they differ:
`bytes` is 8th on the wire and 12th in the interface.

```rust
/// `MCP_TRACE_SCHEMA_VERSION` (`mcp-trace.ts:7`).
pub const MCP_TRACE_SCHEMA_VERSION: u8 = 1;

/// `McpTraceEvent` in `createMcpTraceEvent`'s insertion order.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTraceEvent {
    /// Literal `1`.
    pub version: u8,
    /// `new Date().toISOString()`.
    pub timestamp: String,
    pub direction: TraceDirection,
    /// `redactTraceText(server, 120)`.
    pub server: String,
    pub transport: TraceTransportKind,
    pub kind: TraceMessageKind,
    pub status: TraceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    /// `redactTraceText(message.method, 120)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// **Asymmetry with `related_request_id`, and it is deliberate:** an absent `id` on a message
    /// that HAS an `id` key is written as `null`; `relatedRequestId` is OMITTED when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Option<TraceId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_request_id: Option<TraceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}

/// `traceId(value)` (`mcp-trace.ts:74-79`) over rmcp's `NumberOrString`
/// (`rmcp-3.1.4/src/model.rs:229-234`).
///
/// A STRING id becomes the literal `"[REDACTED_ID]"` — an opaque correlation token can itself be a
/// secret. Numeric ids pass through, because correlating a request with its response is the whole
/// point of writing an id at all. The `Number.isFinite` arm has no Rust counterpart: `i64` is always
/// finite, so `?? null` is unreachable.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TraceId {
    Number(i64),
    Redacted(&'static str),
}
```

Build the event from **one** serialisation pass — that is simultaneously `messageBytes`
(`mcp-trace.ts:81-87`), `messageKind` (`:69-72`) and the `method`/`id`/`error.code` reads, and it is
the only way to reach the `method` string generically across rmcp's `ClientRequest`/`ServerNotification`
enums:

```rust
/// `createMcpTraceEvent` (`mcp-trace.ts:89-120`), over one `serde_json::to_vec`.
///
/// `messageKind` is `"method" in message ? ("id" in message ? request : notification) : response` —
/// so rmcp's distinct `JsonRpcMessage::Error` variant (`model.rs:685`) is a **response**, and its
/// `error.code` is the `errorCode` field.
pub fn trace_event<M: Serialize>(
    direction: TraceDirection,
    server: &str,
    transport: TraceTransportKind,
    message: &M,
    status: TraceStatus,
    duration: Option<Duration>,
) -> McpTraceEvent { /* … */ }
```

### 6.2 Redaction (`MCP-474`)

Four `LazyLock<Regex>`, all lookaround-free, so the linear-time engine suffices and `fancy-regex` is
not needed. `regex` is already a dependency ([cyrup-mcp/Cargo.toml:107](../../crates/cyrup-mcp/Cargo.toml))
with `LazyLock<Regex>` precedent at [agent_plugin.rs:149](../../crates/cyrup-mcp/src/agent_plugin.rs).

```rust
/// `redactTraceText(value, maxLength = 160)` (`mcp-trace.ts:57-67`). Both call sites pass 120.
///
/// Two exactness traps, both load-bearing:
///
/// 1. The third replacement's JS replacement string is `"$1=[REDACTED]"` against a **non-capturing**
///    `(?:…)` group, so JS emits the LITERAL `$1`. Rust's `Regex::replace_all` would interpolate a
///    group that does not exist and emit nothing. Port it as `${nothing}1=[REDACTED]`-free literal:
///    use `regex::NoExpand("$1=[REDACTED]")`.
/// 2. Truncation is `slice(0, maxLength - 1) + "…"` in **UTF-16 code units**, not chars and not
///    bytes. `char_indices` over a UTF-16 running count, the same accounting `truncate_at_word`
///    (registration.rs:571) already does.
pub fn redact_trace_text(value: &str, max_length: usize) -> String { /* … */ }
```

### 6.3 The writer (`MCP-476`, `MCP-477`, `MCP-481`)

Carry the injectable-fs seam. Without it the truncate-then-append ordering and the byte-cap latch are
not observable in-crate at all.

```rust
/// `McpTraceWriterOptions.{appendFile,writeFile,mkdir}` (`mcp-trace.ts:46-48`) as one trait.
pub trait TraceFs: Send + Sync + 'static {
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()>;
    fn truncate(&self, path: &Path) -> std::io::Result<()>;
    fn append(&self, path: &Path, line: &str) -> std::io::Result<()>;
}

/// `McpTraceWriter` (`mcp-trace.ts:122-200`).
///
/// Every latch is one-way and every failure path is silent: **tracing must never change MCP
/// request/response behaviour**, which is the property the whole type is shaped around. `write` is
/// sync and infallible; the queue is a `tokio::sync::Mutex` held across the append so lines cannot
/// reorder — upstream's single promise chain, with the same guarantee.
pub struct TraceWriter {
    path: PathBuf,
    /// `boundedPositiveInteger(maxBytes, DEFAULT_MCP_TRACE_MAX_BYTES)` — `positive_int`
    /// (config.rs:1063) is already that predicate.
    max_bytes: u64,
    max_events: u64,
    fs: Arc<dyn TraceFs>,
    state: Mutex<TraceWriterState>, // bytes_written, events_written, disabled, initialized
}
```

`write(&self, event: &McpTraceEvent)` in upstream's exact order: `disabled || events >= max_events`
→ return; serialise (a failure **latches disabled**); `bytes > max_bytes - bytes_written` → **latch
disabled** and return (note the subtraction order — a line that would overflow disables the writer
rather than being skipped); increment **both counters before** enqueueing; append.

Path derivation (`createMcpTraceWriter`, `mcp-trace.ts:202-217`), with `.cyrup` already decided and in
the tree ([dirs.rs:116](../../crates/cyrup-mcp/src/dirs.rs), `:203`):

```rust
/// `settings.file` verbatim when absolute, resolved against the session cwd when relative; else
/// `<cwd>/.cyrup/mcp-traces/mcp-<ISO with `:` and `.` → `-`>-<≤8 base36 chars>.jsonl`.
///
/// The random suffix is upstream's `Math.random().toString(36).slice(2, 10)`. `uuid` is already a
/// dependency and its manifest comment (`Cargo.toml:155-157`) names the trace writer as the reason:
/// take `Uuid::now_v7`'s low bits through a base36 encoder rather than adding an RNG.
pub fn trace_file_path(dirs: &ConfigDirs, settings: &TraceSettings, suffix: &str) -> PathBuf { /* … */ }
```

`MCP-478`'s combining function — `??`, never `||`:

```rust
/// `isMcpTraceEnabled(definition, settings)` (`mcp-trace.ts:223-228`):
/// `definition.trace ?? settings?.enabled === true`.
///
/// A per-server `trace: false` beats a global `enabled: true`. `||` would invert that and is the
/// bug this function exists to make un-writable.
#[must_use]
pub fn is_mcp_trace_enabled(entry: &ServerEntry, settings: &McpSettings) -> bool {
    entry.trace.unwrap_or_else(|| settings.trace_enabled())
}
```

`trace_enabled()`/`trace_max_bytes()`/`trace_max_events()` already exist unconsumed at
[config.rs:1267-1281](../../crates/cyrup-mcp/src/config.rs). This function is their first caller, and is what
makes `MCP-481` consumed rather than declared.

### 6.4 The decorator (`MCP-479`)

Upstream patches the transport **in place** because the TS SDK sniffs its concrete type before
connect (`mcp-trace.ts:230-235`). **That constraint does not exist here, and the crate already says
so**: `serve_client_with_lifecycle` runs `discover_startup` on the **same** `&mut transport`, as
[runtime.rs:1141-1150](../../crates/cyrup-mcp/src/runtime.rs) documents at length. A newtype is safe.

```rust
/// `wrapTransportWithMcpTrace` (`mcp-trace.ts:236-297`) as a newtype.
///
/// # The one real consequence, stated where a reader will hit it
///
/// `DynamicTransportError` records `transport_name: T::name()` and `transport_type_id:
/// TypeId::of::<T>()` (`rmcp-3.1.4/src/transport.rs:238-252`), and `is::<T, R>()`/`downcast::<T, R>()`
/// key on both. Wrapping CHANGES the error identity: any downcast on the connect error path must
/// target `TracingTransport<T>` or unwrap the inner error first. Nothing in this crate downcasts a
/// transport error today; this doc is the tripwire for the day something does.
pub struct TracingTransport<T> {
    inner: T,
    server: Arc<str>,
    kind: TraceTransportKind,
    writer: Arc<TraceWriter>,
}

impl<T: Transport<RoleClient>> Transport<RoleClient> for TracingTransport<T> {
    type Error = T::Error;

    /// The returned future is `'static`, so the event's metadata is computed HERE, from `&item`,
    /// before `item` is moved into the inner send. Timing brackets the inner send only.
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let sent = trace_event(TraceDirection::Outbound, &self.server, self.kind, &item, TraceStatus::Sent, None);
        let mut failed = sent.clone();
        failed.status = TraceStatus::Error;
        let writer = Arc::clone(&self.writer);
        let started = Instant::now();
        let inner = self.inner.send(item);
        async move {
            let outcome = inner.await;
            let elapsed = started.elapsed();
            // `Math.max(0, Math.round(ms * 100) / 100)` — two decimal places, and the rounding is
            // observable in a golden line.
            let mut event = if outcome.is_ok() { sent } else { failed };
            event.duration_ms = Some(round_2dp(elapsed.as_secs_f64() * 1000.0));
            writer.write(&event);
            // Rethrow unchanged. Tracing observes; it never converts.
            outcome
        }
    }

    /// `receive` replaces upstream's `onmessage` interception — rmcp pulls where the TS SDK pushes,
    /// so there is no property to define and the `defineProperty` try/catch has no analogue.
    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        async move {
            let message = self.inner.receive().await;
            if let Some(message) = message.as_ref() {
                self.writer.write(&trace_event(
                    TraceDirection::Inbound,
                    &self.server,
                    self.kind,
                    message,
                    TraceStatus::Received,
                    None,
                ));
            }
            message
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}
```

Post-Cut-1/Cut-3 the kind enum is `{Stdio, StreamableHttp, Unknown}` — `sse` and `unix-socket` have
no producer left, and the constructor-name sniffing at `mcp-trace.ts:299-306` existed only to tell the
two HTTP transports apart. Carry the kind as an enum from the construction site; never inspect a type
name.

### 6.5 Lifecycle (`MCP-480`)

Three edits, all named in place by existing comments:

1. **The setter.** [server_manager.rs:1431-1436](../../crates/cyrup-mcp/src/server_manager.rs) is a comment saying
   `setTraceConfig` is absent. Replace it with the field `trace_settings: Mutex<Option<TraceSettings>>`,
   the setter, and `trace_writer: tokio::sync::OnceCell<Arc<TraceWriter>>` — one writer per manager,
   so the byte and event budgets are **session-global**, which is upstream's `this.traceWriter ??=`
   at `server-manager.ts:452-454`. Call it from `initialize_mcp` beside the other four setters.
2. **The hand-off.** Add `pub trace: Option<Arc<TraceWriter>>` to `CreateConnection`
   ([server_manager.rs:1107-1127](../../crates/cyrup-mcp/src/server_manager.rs)) and fill it at the construction
   site `:1815-1822` from `is_mcp_trace_enabled(&definition, settings)`. That is upstream's shape
   exactly — the observer is computed by the *manager* and passed *into* `createConnection`
   (`server-manager.ts:451-457`), so the builder never learns about settings. In `ConnectionBuilder`,
   wrap at the three transport construction sites:
   `connect_stdio`'s `process` ([runtime.rs:2487](../../crates/cyrup-mcp/src/runtime.rs)) and both
   `http_transport_with_client(probe, config)` calls (`:2800`, `:2815`). One instrumentation point per
   kind means upstream's `transportAlreadyTraced` flag collapses to a single decision.
3. **The flushes.** `dispose_connection` ([server_manager.rs:2339-2348](../../crates/cyrup-mcp/src/server_manager.rs))
   — `tokio::join!(connection.dispose(), writer.flush())`, both failures aggregating into
   `CONNECTION_CLEANUP_FAILED`, which is upstream's `Promise.allSettled` +
   `AggregateError(failures, "MCP connection cleanup failed")` at `server-manager.ts:1133-1140`. And
   `close_all_inner` `:2479` — the final `flush().await` after the configs are nulled, where the
   comment already marks the spot.

---

## 7. Order of work

```
§6 trace.rs ──────────────────────────────── independent, start now
§2 wave 0 (handler factory) ─┬─> §3 sampling.rs
                             └─> §5 elicitation.rs (needs §4 schema.rs, and §5.1 first)
§4 schema.rs ────────────────────────────── independent; also discharges MCP-092
```

Two sequencing rules that are not negotiable:

* **§5.1 before §5.2.** `McpDialog`'s `input`/`notify` arms must exist before the field loop is
  written, or the loop grows its own dialog path and MCP-471's invariant — *the only
  `HostServices::{confirm,select,input}` calls in the crate are inside `McpDialog`* — is broken
  silently. It currently holds with no bypass ([owner.rs:577](../../crates/cyrup-mcp/src/owner.rs), `:587`).
* **§4 before §5.3.** Writing the coercion pass against an ad-hoc validator is how the crate ends up
  with two that disagree about `format`.

`MCP-232` (13e) is the other `McpDialog` consumer; whoever lands §5.1 owns the arm's shape for both.

---

## 8. Findings relayed, not built

The verification units are test/CI/doc deliverables and are **not** part of this task. Their corrected
verdicts, for whoever schedules them:

| unit | corrected finding |
|---|---|
| `MCP-483` / `MCP-494` | there is still no `.github/` directory in the repository at all |
| `MCP-484` | the hidden-subcommand pre-dispatch pattern to copy exists twice: [subagent_runner_cmd.rs:61](../../crates/cyrup/src/subagent_runner_cmd.rs) + [main.rs:115](../../crates/cyrup/src/main.rs), and [intercom_broker_cmd.rs:36](../../crates/cyrup/src/intercom_broker_cmd.rs) + [main.rs:131](../../crates/cyrup/src/main.rs) |
| `MCP-486` | no baseline file exists to have been copied — the safe starting state |
| `MCP-491` | **corrected count (C-7):** `crates/*/tests/*.rs` matches **11** files across five crates, not 12; only `cyrup-it` sets `autotests = false`, so the true `[[test]]` count is **19**, not 20, against G2's cap of 7 ([TEST-ARCHITECTURE.md:1118-1121](../../docs/TEST-ARCHITECTURE.md)). The `mcp` target already exists ([cyrup-it/Cargo.toml:198-204](../../crates/cyrup-it/Cargo.toml) → [tests/mcp/main.rs](../../crates/cyrup-it/tests/mcp/main.rs)), so the unit is "reconcile two guardrails with a tree that already breaks both", not "find a home" |
| `MCP-492` | citation drifted: the test is [oauth.rs:4655](../../crates/cyrup-mcp/src/oauth.rs) `the_callback_listener_end_to_end`, not `:4797` |
| `MCP-495` | `cyrup-test-support` has **no `env` module**: `crates/cyrup-test-support/src/` is `auth, differential, golden, harness, interop, lib, messages, response, scripted, tempdir, tool_ext, tree, tui`. Both doc references ([TEST-ARCHITECTURE.md:650-657](../../docs/TEST-ARCHITECTURE.md) and the `clippy.toml` `disallowed-methods` reason string) are dangling |
| `MCP-498` | the child-process harness's cold-cache case needs `install_surface_sync` to have a caller (**C-6**), which it does not. The harness's binary-resolution convention exists: `CYRUP_IT_BIN_*` from [cyrup-it/build.rs:190-197](../../crates/cyrup-it/build.rs); `env!("CARGO_BIN_EXE_…")` is banned by G4 |
| `MCP-499` | the spec at [13i:1658](../../docs/gap-analysis/13i-mcp-protocol-and-verification.md) names `run_differential` as existing. It does not — [differential.rs](../../crates/cyrup-test-support/src/differential.rs) provides `diff_sequences` (`:36`), `normalized_jsonl` (`:56`), `diff_normalized` (`:71`) and `canonicalize_cross_impl` (`:96`) |

Ledger corrections to relay (findings, **not** edits to make — nothing under `docs/` is touched by
this task): `MCP-455` → implemented; `MCP-469`/`MCP-122` → the registry exists on the manager;
`MCP-468`'s cited evidence is stale (**C-8**); `MCP-467` has no open ruling (**C-2**); `MCP-465` needs
no version bump (**C-4**).

---

## 9. Definition of Done

Structural, greppable, and checkable without running a suite.

1. `cargo check --workspace --all-targets` exits 0 and `cargo doc --workspace --no-deps --bins`
   exits 0 (`--document-private-items` is on; `rustdoc::broken_intra_doc_links` is `deny`).
2. `crates/cyrup-mcp/src/` contains **four new modules** — `sampling.rs`, `elicitation.rs`,
   `schema.rs`, `trace.rs` — each declared in `lib.rs` and each opening with a module doc that cites
   its upstream `file:line`.
3. **`with_handler_factory` has a caller.** `grep -rn "with_handler_factory" crates/` shows the
   definition plus the `Arc::new_cyclic` call in `initialize_mcp`. Likewise
   `set_sampling_config`, `set_elicitation_config` and the new `set_trace_config` each have exactly
   one production caller in `runtime.rs`.
4. **No hook type is left unproduced.** `SamplingHook`, `ElicitationHook`, `ElicitationCompleteHook`
   and `NotifyHook` each have a producer outside `#[cfg(test)]`.
5. `sampling.rs` re-exports MCP-455's existing items from `owner.rs` rather than redefining them:
   `grep -c "SAMPLING_REQUEST_APPROVAL_TITLE" crates/cyrup-mcp/src/*.rs` shows the definition only in
   `owner.rs`.
6. **The one ordered read.** `grep -rn "\.properties" crates/cyrup-mcp/src/elicitation.rs` shows every
   iteration going through `ordered_properties`; no `schema.properties.iter()` at a call site.
7. **One validator.** `grep -rn "jsonschema::" crates/cyrup-mcp/src/` matches only `schema.rs`.
8. **One dialog route.** `grep -rn "services\.\(confirm\|select\|input\)\|HostServices::\(confirm\|select\|input\)"
   crates/cyrup-mcp/src/` matches only inside `McpDialog` (`owner.rs`).
9. The tracer is wired at all three points: `set_trace_config` called from `initialize_mcp`;
   `CreateConnection::trace` filled at `server_manager.rs:1815`; `TracingTransport` constructed at
   `connect_stdio` and at both `http_attempt` arms; `flush()` awaited in `dispose_connection` and at
   the end of `close_all_inner`.
10. Every literal string ported from upstream is a `pub const` or a `pub fn …_message(…)` carrying its
    `file:line`, including the ones with no reachable throw site (`SAMPLING_TASKS_UNSUPPORTED`), which
    carry a comment saying **why** they have none.
11. No file under `docs/` and no file under `tmp/` is modified.

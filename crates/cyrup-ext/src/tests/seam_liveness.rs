//! Liveness of the host/extension seams — the "written into a map with no reader" defect class.
//!
//! Every test here fails against the pre-fix tree, except the one explicitly labelled as
//! characterizing previously-untested accessors. They cover the seams that each accepted a
//! registration, returned success, and then dropped it:
//!
//! * EXT-058 — a guest's `registration.subscribe` / `register-command` from a LIVE handler
//!   (upstream `api.on` / `registerCommand` are re-read at every dispatch: `runner.ts:806`,
//!   `runner.ts:647-649` @v0.83.0).
//! * EXT-059 / MCP-037a — `refresh_tools` reporting "nothing changed" for a registration it did
//!   not itself re-wrap, while `take_tools_dirty` destroyed the signal.
//! * EXT-060 — the native rich-ctx enrichment existing on only ONE arm of `wasm-host`.
//! * The dispatcher's poisoned-lock fail-soft, which silently disabled every extension event.
//!
//! Feature-independent on purpose: everything except the two guest-import tests runs on BOTH
//! `cargo test -p cyrup-ext` and `cargo test -p cyrup-ext --no-default-features`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{
    CommandDescriptor, EventKind, ExtError, ExtKind, Extension, ExtensionHost, ExtensionRegistry,
    HookOutcome, HostConfig, HostCtx, HostCtxRich, HostCtxSource, HostEvent, InitApi,
    NativeExtension, Subscriptions, ToolDescriptor,
};
use cyrup_core::{
    CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// EXT-058 — a subscription taken AFTER load must be honoured on the next event.
// ---------------------------------------------------------------------------

/// An extension whose subscription set is LIVE, the shape `LiveExtension` now has: the `subscribe`
/// host import mutates it while the extension is loaded. Upstream's `api.on(event, handler)`
/// mutates `extension.handlers` the same way and every emitter re-reads it
/// (`extensions/loader.ts:252-258`, `runner.ts:806` @v0.83.0).
struct LateSubscriber {
    id: ExtensionId,
    subs: Mutex<Subscriptions>,
    seen: AtomicUsize,
}

impl LateSubscriber {
    fn new() -> Self {
        Self {
            id: ExtensionId::from("late-subscriber"),
            subs: Mutex::new(Subscriptions::empty()),
            seen: AtomicUsize::new(0),
        }
    }

    /// The guest's `registration.subscribe` import, after `init`.
    fn subscribe_now(&self, kind: EventKind) {
        if let Ok(mut g) = self.subs.lock() {
            g.add(kind);
        }
    }

    fn seen(&self) -> usize {
        self.seen.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl Extension for LateSubscriber {
    fn id(&self) -> &ExtensionId {
        &self.id
    }
    fn kind(&self) -> ExtKind {
        ExtKind::Wasm
    }
    fn subscriptions(&self) -> Subscriptions {
        self.subs.lock().map(|g| *g).unwrap_or_else(|_| Subscriptions::empty())
    }
    async fn invoke_event(
        &self,
        _ev: &HostEvent,
        _cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        self.seen.fetch_add(1, Ordering::Relaxed);
        Ok(HookOutcome::Noop)
    }
}

#[tokio::test]
async fn a_subscription_taken_after_load_receives_the_next_event() {
    let host = ExtensionHost::new(HostConfig::default());
    let ext = Arc::new(LateSubscriber::new());
    host.dispatcher().add(ext.clone()).expect("add");
    let cancel = CancelToken::new();

    // Nothing subscribed yet: the cheap gate must still skip the handler loop.
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;
    assert_eq!(ext.seen(), 0, "an unsubscribed extension must not be invoked");

    // The late `subscribe` — pi's `pi.on(\"agent_start\", …)` from a live handler.
    ext.subscribe_now(EventKind::AgentStart);
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;
    assert_eq!(
        ext.seen(),
        1,
        "a subscription taken after load must be honoured on the next event — the dispatcher's \
         subscriber set is re-read per dispatch, never frozen at load"
    );
}

// ---------------------------------------------------------------------------
// EXT-058 — the guest registration imports write THROUGH to the shared registry.
// ---------------------------------------------------------------------------

/// `registration.register-command` called from a LIVE handler (i.e. any time other than the
/// instant after `init`) must land in the registry that `command_route` /
/// `AgentSession::try_execute_wasm_command` read. It used to stage into a per-guest buffer that
/// `LiveExtension::load` drained exactly once, so a post-`init` registration was accepted and then
/// invisible forever.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_command_registered_after_init_is_routable() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;
    use crate::host::{GuestState, HostState, StoreLimits};

    let registry = Arc::new(ExtensionRegistry::new());
    let owner = ExtensionId::from("guest-a");
    let guest = Arc::new(GuestState::new(owner.clone(), registry.clone()));
    let mut state = HostState::with_guest(StoreLimits::default(), guest);

    // No `LiveExtension::load` drain runs here — this is the live-handler path.
    state
        .register_command("late".into(), json!({"description": "registered late"}).to_string())
        .await;

    assert_eq!(
        registry.resolved_command_owner("late").expect("resolve"),
        Some(owner),
        "a command registered from a live handler must be routable, exactly as pi's \
         `getCommand` re-reads `extension.commands` at every dispatch"
    );
}

/// The same seam for `subscribe`: the import writes the live bitset the dispatcher now reads.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn a_guest_subscribe_after_init_updates_the_live_bitset() {
    use crate::host::live::bindings::cyrup::ext::registration::Host as RegistrationHost;
    use crate::host::{GuestState, HostState, StoreLimits};

    let registry = Arc::new(ExtensionRegistry::new());
    let guest = Arc::new(GuestState::new(ExtensionId::from("guest-b"), registry));
    let mut state = HostState::with_guest(StoreLimits::default(), guest.clone());

    assert!(guest.subscriptions().is_empty());
    state.subscribe(vec![EventKind::ToolCall as u8]).await;
    assert!(
        guest.subscriptions().contains(EventKind::ToolCall),
        "`<LiveExtension as Extension>::subscriptions` reads exactly this bitset per dispatch"
    );
}

/// pi's per-extension `commands` is a `Map`, so the SAME extension re-registering a name replaces
/// its entry (`extensions/loader.ts:270-277` @v0.83.0). cyrup's `command_order` is a
/// cross-extension Vec; a blind push made the extension's own re-registration look like two
/// different extensions claiming one name, which `resolved_commands` disambiguates into
/// `deploy:1`/`deploy:2` — un-routing the bare `/deploy` the extension still expects to own.
#[test]
fn an_owner_re_registering_its_own_command_replaces_it() {
    let registry = ExtensionRegistry::new();
    let owner = ExtensionId::from("guest-a");
    registry
        .register_command(
            owner.clone(),
            "deploy",
            CommandDescriptor { description: "v1".into(), completions: vec![] },
        )
        .expect("first");
    registry
        .register_command(
            owner.clone(),
            "deploy",
            CommandDescriptor { description: "v2".into(), completions: vec![] },
        )
        .expect("second");

    let resolved = registry.resolved_commands().expect("resolve");
    assert_eq!(resolved.len(), 1, "one upstream Map entry, one row: {resolved:?}");
    assert_eq!(resolved[0].invocation_name, "deploy", "the bare name stays routable");
    assert_eq!(resolved[0].descriptor.description, "v2", "the newer descriptor wins");
}

// ---------------------------------------------------------------------------
// EXT-059 / MCP-037a — `refresh_tools` must report a registration it did not re-wrap.
// ---------------------------------------------------------------------------

struct StubTool {
    name: String,
    schema: Value,
}

#[async_trait::async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    fn description(&self) -> &str {
        "stub"
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult { content: vec![Content::text("stub")], ..Default::default() })
    }
}

fn descriptor(name: &str, description: &str) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        label: name.into(),
        description: description.into(),
        parameters: json!({"type": "object", "properties": {}}),
        execution_mode: None,
        prompt_snippet: None,
        prompt_guidelines: vec![],
        has_renderer: false,
        prepare_arguments: false,
        render_shell: None,
        constrained_sampling: None,
    }
}

/// The native half of MCP-037a: `register_late_tool` lands in the executable `tools` map, which
/// the `wasm-host` materializer never looks at — so `refresh_tools` reported `Ok(false)` while
/// `take_tools_dirty()` (a `swap(false)`) destroyed the only record that anything had happened.
/// `AgentSession::refresh_extension_tools` returns early on that `false` with no diagnostic.
#[tokio::test]
async fn refresh_tools_reports_a_late_native_registration() {
    let host = ExtensionHost::new(HostConfig::default());
    assert!(!host.refresh_tools().expect("clean"), "nothing registered yet");

    host.register_late_tool(
        ExtensionId::from("native-a"),
        Arc::new(StubTool { name: "late_tool".into(), schema: json!({"type": "object"}) }),
    )
    .expect("register");

    assert!(
        host.refresh_tools().expect("refresh"),
        "a registration that landed since the last refresh must be reported as a change — pi's \
         `registerTool` ends with an unconditional `runtime.refreshTools()`"
    );
    assert!(!host.refresh_tools().expect("refresh"), "and the flag is consumed exactly once");
}

/// The guest half: a descriptor REPLACED by its own owner (the `dynamic-tools.ts` pattern — same
/// name, changed schema/description) marks the set dirty, and that change must reach the caller
/// that gates the model-facing rebuild. It previously did not: the materializer skipped every name
/// that already had an executable counterpart, so the replacement died in `guest_tools` while
/// `WasmTool` kept executing under the descriptor it captured by value.
#[tokio::test]
async fn refresh_tools_reports_a_replaced_guest_descriptor() {
    let host = ExtensionHost::new(HostConfig::default());
    let owner = ExtensionId::from("guest-a");
    host.registry().register_guest_tool(owner.clone(), descriptor("t", "v1")).expect("first");
    assert!(host.refresh_tools().expect("refresh"), "the first registration is a change");

    host.registry().register_guest_tool(owner, descriptor("t", "v2")).expect("replace");
    assert!(
        host.refresh_tools().expect("refresh"),
        "re-registering the SAME name with a CHANGED descriptor is a change too"
    );
    // The registry-facing view (pi's `getAllTools`) and the refresh signal must agree.
    let info = host.registry().tool_info().expect("tool info");
    assert!(
        info.iter().any(|t| t["description"] == json!("v2")),
        "the replaced descriptor is what the registry serves: {info:?}"
    );
}

/// The two zero-caller enumeration accessors an MCP client is most likely to reach for first.
/// Both were read and found correct; this pins the behaviour they will be trusted for —
/// registration ORDER, no duplicates on a same-owner re-registration, and the two tables staying
/// separate (`extension_tools` reads `tool_order`+`tools`, `guest_tool_descriptors` reads
/// `guest_tool_order`+`guest_tools`).
#[test]
fn the_tool_enumeration_accessors_report_registration_order_without_duplicates() {
    let registry = ExtensionRegistry::new();
    let owner = ExtensionId::from("guest-a");
    for name in ["b_tool", "a_tool"] {
        registry
            .register_tool(
                owner.clone(),
                Arc::new(StubTool { name: name.into(), schema: json!({"type": "object"}) }),
            )
            .expect("register");
    }
    // A same-owner re-registration REPLACES; it must not append a second `tool_order` entry.
    registry
        .register_tool(
            owner.clone(),
            Arc::new(StubTool { name: "b_tool".into(), schema: json!({"type": "object"}) }),
        )
        .expect("re-register");

    let tools = registry.extension_tools().expect("tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names, vec!["b_tool", "a_tool"], "registration order, no duplicate row");

    registry.register_guest_tool(owner, descriptor("g_tool", "v1")).expect("guest tool");
    let guest: Vec<String> =
        registry.guest_tool_descriptors().expect("descs").into_iter().map(|d| d.name).collect();
    assert_eq!(guest, vec!["g_tool".to_string()], "the guest table is separate from `tools`");
}

// ---------------------------------------------------------------------------
// EXT-060 — the native rich ctx must be live on BOTH arms of `wasm-host`.
// ---------------------------------------------------------------------------

/// A feature-independent [`HostCtxSource`]: on the shipped arm the host builds one of these out of
/// the injected `HostServices`, but the trait — and therefore the enrichment — exists with or
/// without the Wasmtime host compiled in. Every value below is the OPPOSITE of
/// `HostCtxRich::default()`, so the assertions cannot be satisfied by the defaulted ctx a
/// `--no-default-features` build used to hand every native built-in.
struct LiveState;

impl HostCtxSource for LiveState {
    fn rich(&self) -> HostCtxRich {
        HostCtxRich {
            model: Some("live-model".into()),
            is_idle: true,
            is_project_trusted: true,
            context_usage: Some(json!({"used": 1})),
            system_prompt: Some("LIVE-SYSTEM-PROMPT".into()),
        }
    }
}

#[derive(Default)]
struct CtxProbe {
    seen: Mutex<Vec<String>>,
}

impl CtxProbe {
    fn snapshot(ctx: &HostCtx) -> String {
        format!("idle={} trusted={}", ctx.is_idle(), ctx.is_project_trusted())
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl NativeExtension for CtxProbe {
    fn id(&self) -> ExtensionId {
        "ctx-probe".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentStart]);
        api.register_command("probe", CommandDescriptor::default());
        api.register_shortcut("ctrl+p", None);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        if let Ok(mut g) = self.seen.lock() {
            g.push(format!("event:{}", Self::snapshot(ctx)));
        }
        HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        _name: &str,
        _args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        Ok(Some(Self::snapshot(ctx)))
    }
    async fn execute_shortcut(&self, _key: &str, ctx: &HostCtx) -> Result<(), ExtError> {
        if let Ok(mut g) = self.seen.lock() {
            g.push(format!("shortcut:{}", Self::snapshot(ctx)));
        }
        Ok(())
    }
}

#[tokio::test]
async fn a_native_reads_live_ctx_state_on_every_arm() {
    let host = ExtensionHost::new(HostConfig::default());
    host.set_ctx_source(Arc::new(LiveState));
    let probe = Arc::new(CtxProbe::default());
    host.load_native(probe.clone()).await.expect("load native");

    let cancel = CancelToken::new();
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;
    let command = host
        .execute_native_command("probe", "", &cancel)
        .await
        .expect("routed")
        .expect("owned")
        .expect("handler ok");
    host.run_shortcut("ctrl+p", &cancel).await.expect("shortcut");

    let seen = probe.seen();
    assert_eq!(
        seen,
        vec!["event:idle=true trusted=true", "shortcut:idle=true trusted=true"],
        "event and shortcut handlers must read the live source, not HostCtxRich::default() — on \
         the native-only build too"
    );
    assert_eq!(
        command.as_deref(),
        Some("idle=true trusted=true"),
        "and so must a command handler"
    );
}

// ---------------------------------------------------------------------------
// A poisoned dispatcher lock disables every extension event — say so, once.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_poisoned_dispatcher_lock_is_reported_once() {
    let host = ExtensionHost::new(HostConfig::default());
    let ext = Arc::new(LateSubscriber::new());
    ext.subscribe_now(EventKind::AgentStart);
    host.dispatcher().add(ext.clone()).expect("add");
    assert!(!host.dispatcher().poison_reported());

    host.dispatcher().poison_for_test();

    let cancel = CancelToken::new();
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;
    assert_eq!(ext.seen(), 0, "fail-soft: a poisoned lock never crashes the host");
    assert!(
        host.dispatcher().poison_reported(),
        "…but it is no longer silent: poisoning is permanent, so every extension event is dropped \
         for the rest of the process and the operator has to be able to see that"
    );
}

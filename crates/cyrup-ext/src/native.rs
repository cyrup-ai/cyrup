//! The native built-in extension path (arch-08 §3.2, R-ARCH-EXT-003). Compiled-in Rust extensions
//! implement [`NativeExtension`]; the same dispatch/registration machinery drives them WITHOUT any
//! wasm. A native built-in and a WASM component are interchangeable at the dispatch layer (both
//! become an [`Extension`] handle); only the call mechanism differs.

use crate::contract::HookOutcome;
use crate::error::ExtError;
use crate::event::{EventKind, HostEvent, Subscriptions};
use crate::extension::{ExtKind, Extension};
use crate::registry::CommandDescriptor;
use cyrup_core::{CancelToken, ExtensionId, Tool};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Which context tier a handler runs in (arch-08 §6.3, the deadlock rule). Session-mutating control
/// ops are legal only from [`CtxTier::Command`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtxTier {
    Event,
    Command,
}

/// The runtime mode the host is in (arch-08 §6.3); UI degrades by mode (R-08-023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExtMode {
    #[default]
    Tui,
    Rpc,
    Json,
    Print,
}

/// Coordinates a sanctioned human-latency wait between a native `on_event` handler and the dispatch
/// invocation budget (P-3, `spec/extensions/cyrup-permission-system-port.md §4`). The native
/// dispatcher wraps every handler in a `DEFAULT_INVOKE_BUDGET` `tokio::time::timeout` and, on expiry,
/// SKIPS the handler and PROCEEDS the action (`dispatch.rs`) — which for a permission gate's
/// `before_tool_call` `ask` is **fail-OPEN**: a human who takes longer than the budget to answer would
/// let the tool run ungated. A handler that must block on a human calls [`HostCtx::begin_human_wait`]
/// to hold a [`HumanWaitGuard`] across the blocking call; while any guard is alive the dispatcher's
/// budget watchdog ([`crate::dispatch::Dispatcher`]) is SUSPENDED — the native analog of the wasm epoch
/// forgiveness the guest UI round-trip already has (arch-08 §6.5a). The wait stays bounded by the
/// handler's OWN timeout (which fail-CLOSES to `Block`), so forgiveness is never an unbounded hang.
/// Reentrant: a counter admits nested/back-to-back guards; the budget resumes only once it returns to
/// 0. This is **permission-only** by construction — no other handler obtains a guard, so every other
/// handler keeps the exact fail-fast budget behavior (a cooperative runaway that never begins a human
/// wait is still timed out).
#[derive(Debug, Default)]
pub struct HumanWaitGate {
    /// Number of live [`HumanWaitGuard`]s (a human wait is in progress while `> 0`). The dispatcher's
    /// budget watchdog polls [`Self::is_waiting`] whenever the budget deadline elapses: while it holds,
    /// the watchdog re-arms the deadline instead of firing (the budget clock only advances when NOT
    /// waiting) — critically WITHOUT ever suspending the handler future itself, so the very call that
    /// will drop the guard keeps running.
    waiting: AtomicUsize,
}

impl HumanWaitGate {
    /// True while at least one [`HumanWaitGuard`] is alive (a human wait is in progress right now).
    pub fn is_waiting(&self) -> bool {
        self.waiting.load(Ordering::Acquire) > 0
    }

    fn begin(self: &Arc<Self>) -> HumanWaitGuard {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        HumanWaitGuard { gate: Arc::clone(self) }
    }
}

/// RAII guard for a sanctioned human wait (see [`HumanWaitGate`]). While held, the dispatch budget is
/// suspended; on drop (including during a panic unwind) it decrements the wait count so the budget
/// resumes for the handler's (instant) post-decision wrap-up.
#[must_use = "the dispatch budget is only suspended while the guard is held"]
pub struct HumanWaitGuard {
    gate: Arc<HumanWaitGate>,
}

impl Drop for HumanWaitGuard {
    fn drop(&mut self) {
        self.gate.waiting.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Context handed to an extension at dispatch (arch-08 §6.3). Event handlers get an `Event`-tier
/// ctx (no session mutation); command handlers get a `Command`-tier ctx. The host check is
/// authoritative even though the SDK also enforces it at the type level.
#[derive(Clone, Debug)]
pub struct HostCtx {
    pub mode: ExtMode,
    pub has_ui: bool,
    pub cwd: PathBuf,
    tier: CtxTier,
    /// Rich native-ctx fields (Pi `ExtensionContext`, types.ts:300-333). On the wasm path these are
    /// served by the `session`/`models`/`ui` capability imports; the native built-in path carries
    /// them inline so a built-in reaches the same surface without crossing a boundary (gap-08 #6).
    rich: HostCtxRich,
    /// The sanctioned-human-wait coordinator (P-3). Shared (via `Arc`) with the dispatcher's budget
    /// watchdog through [`Extension::human_wait_gate`], so a handler's [`Self::begin_human_wait`] and
    /// the watchdog consult the SAME gate. One per handler ctx.
    human_wait: Arc<HumanWaitGate>,
}

/// The richer fields a native built-in's [`HostCtx`] exposes (Pi `ExtensionContext`, types.ts:300-333):
/// the current model, idle/trust flags, the context-usage snapshot, and the active system prompt.
/// (`sessionManager`/`modelRegistry` remain seam-injected handles — arch-08 §5.6 — so they are not
/// inlined here; the data fields a built-in actually reads are.)
#[derive(Clone, Debug, Default)]
pub struct HostCtxRich {
    /// The current model ref (Pi `ctx.model`).
    pub model: Option<String>,
    /// Whether the agent is idle (Pi `ctx.isIdle`).
    pub is_idle: bool,
    /// Whether the project is trusted (Pi `ctx.isProjectTrusted`).
    pub is_project_trusted: bool,
    /// The context-usage snapshot (Pi `ctx.getContextUsage()`).
    pub context_usage: Option<serde_json::Value>,
    /// The active system prompt (Pi `ctx.getSystemPrompt()`).
    pub system_prompt: Option<String>,
}

impl HostCtx {
    /// An event-tier context (inside the agent/session flow): NO session mutation.
    pub fn event(mode: ExtMode, has_ui: bool, cwd: PathBuf) -> Self {
        Self {
            mode,
            has_ui,
            cwd,
            tier: CtxTier::Event,
            rich: HostCtxRich::default(),
            human_wait: Arc::new(HumanWaitGate::default()),
        }
    }

    /// A command-tier context (user-initiated, outside the loop): session mutation allowed.
    pub fn command(mode: ExtMode, has_ui: bool, cwd: PathBuf) -> Self {
        Self {
            mode,
            has_ui,
            cwd,
            tier: CtxTier::Command,
            rich: HostCtxRich::default(),
            human_wait: Arc::new(HumanWaitGate::default()),
        }
    }

    /// Attach the rich native-ctx fields (Pi `ExtensionContext`, gap-08 #6).
    pub fn with_rich(mut self, rich: HostCtxRich) -> Self {
        self.rich = rich;
        self
    }

    /// The rich native-ctx fields (model/idle/trust/usage/system-prompt).
    pub fn rich(&self) -> &HostCtxRich {
        &self.rich
    }

    /// The current model ref (Pi `ctx.model`).
    pub fn model(&self) -> Option<&str> {
        self.rich.model.as_deref()
    }
    /// Whether the agent is idle (Pi `ctx.isIdle`).
    pub fn is_idle(&self) -> bool {
        self.rich.is_idle
    }
    /// Whether the project is trusted (Pi `ctx.isProjectTrusted`).
    pub fn is_project_trusted(&self) -> bool {
        self.rich.is_project_trusted
    }
    /// The context-usage snapshot (Pi `ctx.getContextUsage()`).
    pub fn context_usage(&self) -> Option<&serde_json::Value> {
        self.rich.context_usage.as_ref()
    }
    /// The active system prompt (Pi `ctx.getSystemPrompt()`).
    pub fn system_prompt(&self) -> Option<&str> {
        self.rich.system_prompt.as_deref()
    }

    pub fn tier(&self) -> CtxTier {
        self.tier
    }

    /// Enter a sanctioned human-latency wait (P-3): hold the returned [`HumanWaitGuard`] across a
    /// blocking human interaction (the permission gate's `before_tool_call` `ask` dialog) so the
    /// dispatcher's invocation-budget watchdog is suspended and a slow human answer cannot fire the
    /// budget and fail-OPEN the gate. The wait stays bounded by the caller's OWN timeout. Drop the
    /// guard (or let it fall out of scope) the instant the human interaction returns.
    #[must_use = "hold the guard across the human interaction; dropping it immediately does nothing"]
    pub fn begin_human_wait(&self) -> HumanWaitGuard {
        self.human_wait.begin()
    }

    /// The shared [`HumanWaitGate`] backing [`Self::begin_human_wait`] (P-3). The dispatcher reads this
    /// (via [`Extension::human_wait_gate`]) so its budget watchdog consults the SAME gate the handler
    /// signals through.
    pub fn human_wait_gate(&self) -> Arc<HumanWaitGate> {
        Arc::clone(&self.human_wait)
    }

    /// Deadlock guard (R-08-008): returns `Err(ExtError::Deadlock)` if a session-replacement /
    /// turn-starting control op (new-session/switch/fork/navigate/reload/compact/wait-idle/
    /// send-message/send-user-message) is attempted from an event handler. Authoritative regardless
    /// of the guest SDK's types. GAP-11: `set_model`/`set_thinking_level` are EXEMPT — Pi allows them
    /// from any handler (loader.ts:342-354); live.rs queues them unconditionally and they apply at the
    /// store-free turn-boundary drain, so this gate is not consulted for them.
    pub fn require_command_tier(&self) -> Result<(), ExtError> {
        if self.tier == CtxTier::Command {
            Ok(())
        } else {
            Err(ExtError::Deadlock)
        }
    }
}

/// The decomposed result of [`InitApi`]: the declared subscriptions, registered tools, registered
/// `(name, descriptor)` commands, and the tool names / custom types this extension declared a
/// renderer for (EXT-006).
pub(crate) type InitParts = (
    Subscriptions,
    Vec<Arc<dyn Tool>>,
    Vec<(String, CommandDescriptor)>,
    Vec<String>,
    Vec<String>,
);

/// What a native extension declares during [`NativeExtension::init`]: its subscriptions plus any
/// tools/commands/renderers it registers (arch-08 §3.5). Mirrors the guest's registration imports.
#[derive(Default)]
pub struct InitApi {
    subs: Subscriptions,
    tools: Vec<Arc<dyn Tool>>,
    commands: Vec<(String, CommandDescriptor)>,
    tool_renderers: Vec<String>,
    message_renderers: Vec<String>,
}

impl InitApi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the subscription bitset (R-ARCH-EXT-014): which §5 event kinds this extension handles.
    pub fn subscribe(&mut self, kinds: &[EventKind]) {
        for &k in kinds {
            self.subs.add(k);
        }
    }

    /// Register a tool. Overrides a built-in of the same name at the registry (R-08-012).
    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register a command (runs with a command-tier ctx; may call session ops, R-08-016).
    pub fn register_command(&mut self, name: impl Into<String>, desc: CommandDescriptor) {
        self.commands.push((name.into(), desc));
    }

    /// Declare that this extension renders CUSTOM MESSAGES of `custom_type` (Pi
    /// `api.registerMessageRenderer(customType, renderer)`, extensions/types.ts:1284). The host
    /// routes `render_message_call`/`render_message_result` for that type back to
    /// [`NativeExtension::render_call`]/[`NativeExtension::render_result`]. First registration in
    /// load order wins, matching Pi's `getMessageRenderer` loop (runner.ts:579-587).
    ///
    /// EXT-006: without this a native built-in could not register a renderer at all, which is why
    /// `cyrup-intercom` had to degrade its message card to an unstyled custom entry.
    pub fn register_message_renderer(&mut self, custom_type: impl Into<String>) {
        self.message_renderers.push(custom_type.into());
    }

    /// Declare that this extension renders the TOOL named `tool_name` (Pi's per-tool
    /// `ToolDefinition.renderCall`/`renderResult`, extensions/types.ts:472-481, resolved by
    /// `modes/interactive/components/tool-execution.ts:81-112`). The guest path declares the same
    /// thing through `ToolDescriptor.has_renderer`; a native tool is an already-executable
    /// `Arc<dyn Tool>` and has no descriptor, so it declares it here.
    pub fn register_tool_renderer(&mut self, tool_name: impl Into<String>) {
        self.tool_renderers.push(tool_name.into());
    }

    pub fn subscriptions(&self) -> Subscriptions {
        self.subs
    }

    pub(crate) fn into_parts(self) -> InitParts {
        (self.subs, self.tools, self.commands, self.tool_renderers, self.message_renderers)
    }
}

/// Native built-ins implement this directly (arch-08 §3.2). First-party / promoted extensions
/// (R-ARCH-EXT-003/006) live here — full speed, in-process, no serialization.
#[async_trait::async_trait]
pub trait NativeExtension: Send + Sync {
    fn id(&self) -> ExtensionId;
    /// Registers tools/commands + declares subscriptions. Awaited before the extension goes live
    /// (R-08-001).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError>;
    /// Handle one event. Returns this extension's block/mutate/notify contribution.
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome;

    /// Opt in to the PRE-TRUST bootstrap pass, where `project_trust` is asked (EXT-003). Default
    /// `false`, and that default is load-bearing.
    ///
    /// Pi resolves project trust by loading a throwaway extension set first
    /// (`resource-loader.ts:378-399`), taking the verdict, then loading the real set — and its
    /// module cache holds FACTORIES, not instances (`loader.ts:148,414-437`), so the second pass
    /// calls the factory again against a FRESH `Extension` + `ExtensionAPI`. cyrup has no such
    /// re-instantiation for a native: a native built-in is a process-lifetime `Arc<dyn
    /// NativeExtension>` handed to the builder, so running it through the bootstrap pass calls
    /// [`Self::init`] **twice on the very same object**, with the same interior state.
    ///
    /// That is not hypothetical. `cyrup-ext-subagents`' `RegistrationMode::ChildSafe` arm spawns a
    /// detached nested-control-inbox poller straight from `init`; a second `init` would start a
    /// SECOND poller on the same inbox, each with its own private `seen` set, so both would resolve
    /// and write back the same request. Its `Full` arm likewise re-runs sweeps its own comment
    /// documents as "exactly once per process load". And the trigger is the COMMON case: any repo
    /// carrying a `.cyrup/` directory has trust-requiring resources, and a subagent child re-execs
    /// with no `--approve`.
    ///
    /// So the bootstrap pass loads only the natives that answer `true` here. WASM guests are
    /// unaffected and always participate: a guest load builds a fresh instance in a fresh store,
    /// which IS Pi's fresh-`Extension`-per-factory-call semantics.
    ///
    /// **Override this only if [`Self::init`] is idempotent**, because it will run twice on a
    /// trust-requiring project. A native that subscribes to [`crate::EventKind::ProjectTrust`]
    /// without overriding it is warned about at load time — its vote would otherwise be counted
    /// only in the real pass, which happens after trust is already decided.
    fn decides_project_trust(&self) -> bool {
        false
    }

    /// Execute a registered slash command this extension owns (Pi `command.handler(args, ctx)`,
    /// agent-session.ts:1159; R-08-016). `ctx` is **command-tier** (session mutation allowed). The
    /// optional `String` is the command's text output (Pi commands return `void`; cyrup mirrors the
    /// WASM `execute-command` shape so the two paths are interchangeable). The default rejects: a
    /// native built-in that registers a command via [`InitApi::register_command`] MUST override this
    /// to service it. Built-ins that only subscribe to events leave it unimplemented.
    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        Err(ExtError::Component(format!("native extension has no handler for command `{name}`")))
    }

    /// Render a tool CALL / custom MESSAGE this extension declared a renderer for (Pi
    /// `renderCall`, extensions/types.ts:472-473). `key` is the TOOL NAME for a tool renderer
    /// declared via [`InitApi::register_tool_renderer`], or the CUSTOM TYPE for a message renderer
    /// declared via [`InitApi::register_message_renderer`]. `None` (the default) falls the host back
    /// to its own framing — the same degradation a faulting guest renderer gets.
    ///
    /// Sync on purpose: rendering is a pure projection of the payload, it runs on the UI's event
    /// path, and an `async` renderer would let a built-in stall the frame.
    fn render_call(&self, _key: &str, _call: &serde_json::Value) -> Option<serde_json::Value> {
        None
    }

    /// The result-side companion of [`Self::render_call`] (Pi `renderResult`,
    /// extensions/types.ts:475-481).
    fn render_result(&self, _key: &str, _result: &serde_json::Value) -> Option<serde_json::Value> {
        None
    }

    /// Late-bind the live `Arc<dyn HostServices>` backend (reconciliation §2 item 1 / P-1). Called by
    /// [`crate::ExtensionHost::load_native_with_services`] BEFORE [`Self::init`], handing a native
    /// built-in the SAME capability backend the WASM path already receives (via `discover_and_load`).
    /// The default is a no-op — a built-in that needs none simply ignores it. A built-in that DOES
    /// need late, out-of-`HostCtx` reach (a background tokio task that must resolve the live session
    /// id/file, open a dialog, or inject a turn-triggering message) overrides this to STASH the `Arc`
    /// in its own interior-mutable slot (`OnceLock`/`Mutex`). The captured `Arc` is a shared handle to
    /// the one `LiveHostServices` the session late-attaches its manager / ui sink / inject sink to, so
    /// capturing it early (before those attachments) is correct: the built-in observes them through
    /// the `Arc`'s interior mutability when the background task actually runs. Gated on `wasm-host`
    /// because the [`crate::host::HostServices`] trait itself only exists with the capability host.
    #[cfg(feature = "wasm-host")]
    fn set_host_services(&self, _services: Arc<dyn crate::host::HostServices>) {}
}

/// Wraps a `NativeExtension` into the unified [`Extension`] handle, applying panic containment
/// (R-08-036): a panicking handler is caught and surfaced as `ExtError::Panicked`, never crashing
/// the host. The chain then skips it (arch-08 §6.1).
pub struct NativeHandle {
    id: ExtensionId,
    subs: Subscriptions,
    ctx: HostCtx,
    inner: Arc<dyn NativeExtension>,
    /// The live capability backend, when the host was given one ([`crate::ExtensionHost::
    /// load_native_with_services`]). Used to refresh [`HostCtxRich`] on EVERY dispatch (EXT-005):
    /// idle/trust/usage/model/system-prompt are all live values, so a ctx built once at load time
    /// would go stale — and, before EXT-005, `HostCtxRich::default()` meant a native built-in read a
    /// confident `is_idle = false` / `is_project_trusted = false` rather than the truth.
    #[cfg(feature = "wasm-host")]
    services: Option<Arc<dyn crate::host::HostServices>>,
}

impl NativeHandle {
    pub fn new(
        inner: Arc<dyn NativeExtension>,
        subs: Subscriptions,
        ctx: HostCtx,
    ) -> Self {
        let id = inner.id();
        Self {
            id,
            subs,
            ctx,
            inner,
            #[cfg(feature = "wasm-host")]
            services: None,
        }
    }

    /// Attach the live capability backend so each dispatch gets a FRESH [`HostCtxRich`] (EXT-005).
    #[cfg(feature = "wasm-host")]
    #[must_use]
    pub fn with_services(mut self, services: Option<Arc<dyn crate::host::HostServices>>) -> Self {
        self.services = services;
        self
    }

    /// The ctx for one dispatch: the handle's stable base ctx (tier, mode, cwd and — critically —
    /// the SHARED [`HumanWaitGate`] the dispatcher's budget watchdog polls) with the rich fields
    /// re-read from the live backend.
    fn dispatch_ctx(&self) -> HostCtx {
        #[cfg(feature = "wasm-host")]
        if let Some(svc) = &self.services {
            return self.ctx.clone().with_rich(rich_from_services(svc.as_ref()));
        }
        self.ctx.clone()
    }
}

/// Snapshot the Pi `ExtensionContext` data fields (types.ts:329-346) off a live capability backend.
#[cfg(feature = "wasm-host")]
pub(crate) fn rich_from_services(svc: &dyn crate::host::HostServices) -> HostCtxRich {
    HostCtxRich {
        model: svc.current_model(),
        is_idle: svc.is_idle(),
        is_project_trusted: svc.is_project_trusted(),
        context_usage: Some(svc.context_usage()),
        system_prompt: svc.system_prompt(),
    }
}

#[async_trait::async_trait]
impl Extension for NativeHandle {
    fn id(&self) -> &ExtensionId {
        &self.id
    }

    fn kind(&self) -> ExtKind {
        ExtKind::Native
    }

    fn subscriptions(&self) -> &Subscriptions {
        &self.subs
    }

    /// The P-3 human-wait gate for this native handler: its ctx's shared [`HumanWaitGate`]. The
    /// dispatcher's budget watchdog reads it to forgive a sanctioned human wait (see
    /// [`HostCtx::begin_human_wait`]). Only natives that actually block on a human (the permission
    /// gate) ever set it waiting; for every other native it stays idle, preserving the fail-fast budget.
    fn human_wait_gate(&self) -> Option<Arc<HumanWaitGate>> {
        Some(self.ctx.human_wait_gate())
    }

    async fn invoke_event(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        // Containment: catch a panicking handler (R-08-036). `AssertUnwindSafe` is sound here — on
        // a caught unwind we discard the handler's state and surface an error; we never resume it.
        let ctx = self.dispatch_ctx();
        let fut = AssertUnwindSafe(self.inner.on_event(ev, &ctx));
        let raced = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExtError::Cancelled),
            r = fut.catch_unwind() => r,
        };
        match raced {
            Ok(outcome) => Ok(outcome),
            Err(panic) => Err(ExtError::Panicked(panic_msg(panic))),
        }
    }
}

fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

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
    /// The BAG that built [`Self::system_prompt`] — pi `ctx.getSystemPromptOptions()`
    /// (`extensions/types.ts:355` @v0.83.0), shape at `core/system-prompt.ts:8-25`. `None` when no
    /// session backend supplied one; [`HostCtx::system_prompt_options`] then answers pi's own
    /// no-backend default. Present on the NATIVE tier for the same reason `cwd` had to be (EXT-044):
    /// a capability only one of cyrup's two tiers can express is a divergence from cyrup, not just
    /// from pi (EXT-061).
    pub system_prompt_options: Option<serde_json::Value>,
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

    /// The base system-prompt construction options (pi `ctx.getSystemPromptOptions()`,
    /// `extensions/types.ts:355` @v0.83.0) — EXT-061, the native half of the WIT
    /// `ctx-state.get-system-prompt-options` import.
    ///
    /// COMMAND-tier, because that is where upstream declares it: `getSystemPrompt()` is on the base
    /// `ExtensionContext` (`:346`) and this is on `ExtensionCommandContext` (`:353-387`). An
    /// event-tier caller gets [`ExtError::Deadlock`] rather than a bag, matching what the WIT tier
    /// gate hands a guest.
    ///
    /// With no bag attached the answer is pi's own no-backend default — `() => ({ cwd: this.cwd })`
    /// (`core/extensions/runner.ts:287`, re-bound at `:350`) — so a built-in always reads a
    /// well-formed bag, never `{}` and never an "unavailable" it has to special-case.
    pub fn system_prompt_options(&self) -> Result<serde_json::Value, ExtError> {
        self.require_command_tier()?;
        Ok(self.rich.system_prompt_options.clone().unwrap_or_else(|| {
            serde_json::json!({ "cwd": self.cwd.to_string_lossy().into_owned() })
        }))
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
/// `(name, descriptor)` commands, and the tool names / custom message types / custom ENTRY types
/// this extension declared a renderer for (EXT-006, X15).
// The tail six members are EXT-035 / EXT-018: shortcuts `(key, description)`, flags
// `(name, spec)`, provider registrations `(id, config)`, per-command autocomplete opt-ins, the
// count of stacked GLOBAL autocomplete providers, and the inter-extension bus topics this
// extension listens on.
pub(crate) type InitParts = (
    Subscriptions,
    Vec<Arc<dyn Tool>>,
    Vec<(String, CommandDescriptor)>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<(String, Option<String>)>,
    Vec<(String, serde_json::Value)>,
    Vec<(String, serde_json::Value)>,
    Vec<String>,
    u32,
    Vec<String>,
    bool,
    bool,
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
    entry_renderers: Vec<String>,
    // --- EXT-035: the six surfaces `interface registration` offered and `InitApi` did not. pi has
    // ONE extension kind and ONE api object (`extensions/loader.ts:274-410` @v0.83.0 builds a
    // single `ExtensionAPI` carrying registerTool/registerCommand/registerShortcut/registerFlag/
    // getFlag/registerProvider/unregisterProvider/registerMessageRenderer/registerEntryRenderer/
    // addAutocompleteProvider/events and hands it to EVERY extension it loads), so there is no
    // upstream notion of an extension that can register tools but not shortcuts, flags or
    // providers. A native reached 5 of 11.
    shortcuts: Vec<(String, Option<String>)>,
    flags: Vec<(String, serde_json::Value)>,
    providers: Vec<(String, serde_json::Value)>,
    autocomplete: Vec<String>,
    autocomplete_providers: u32,
    /// EXT-018: bus topics, pi's `events` on the same one API object.
    bus_topics: Vec<String>,
    /// EXT-019: whether this extension registered a markdown transformer. A BOOL, not a list,
    /// because upstream stores `extension.markdownTransformer = transformer` — at most one per
    /// extension (`extensions/loader.ts:309-312` @v0.84.1, field at `types.ts:1703`).
    markdown_transformer: bool,
    /// EXT-021: whether this extension subscribed to raw terminal input. A BOOL, not a list, for
    /// the same reason as `markdown_transformer`: upstream's `Set` de-duplicates by handler and a
    /// native has exactly one [`NativeExtension::on_terminal_input`].
    terminal_input: bool,
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
    /// `ToolDefinition.renderCall`/`renderResult`, extensions/types.ts:489-497, resolved by
    /// `modes/interactive/components/tool-execution.ts:81-112`). The guest path declares the same
    /// thing through `ToolDescriptor.has_renderer`; a native tool is an already-executable
    /// `Arc<dyn Tool>` and has no descriptor, so it declares it here.
    pub fn register_tool_renderer(&mut self, tool_name: impl Into<String>) {
        self.tool_renderers.push(tool_name.into());
    }

    /// Declare that this extension renders custom ENTRIES of `custom_type` (Pi
    /// `api.registerEntryRenderer(customType, renderer)`, extensions/types.ts:1295, implemented at
    /// `loader.ts:314-318`). The host routes [`crate::ExtensionHost::render_entry`] for that type
    /// back to [`NativeExtension::render_entry`]. First registration in load order wins, matching
    /// Pi's `getEntryRenderer` loop (runner.ts:593-600).
    ///
    /// X15 — DISTINCT from [`Self::register_message_renderer`]. A custom MESSAGE participates in
    /// LLM context and is drawn by `CustomMessageComponent`, which SWALLOWS a renderer throw and
    /// falls through to its default `[type] body` box (`custom-message.ts:82-84`). A custom ENTRY
    /// is TUI-only durable state (`pi.appendEntry`) drawn by `CustomEntryComponent`, which draws a
    /// `[type] renderer failed: …` box instead (`custom-entry.ts:47-52`) and draws NOTHING at all
    /// when no renderer claims the type (`interactive-mode.ts:3432-3435`).
    pub fn register_entry_renderer(&mut self, custom_type: impl Into<String>) {
        self.entry_renderers.push(custom_type.into());
    }

    /// Register a keyboard shortcut (EXT-035 / EXT-040; pi `registerShortcut(shortcut,
    /// {description?, handler})`, `extensions/types.ts:1250` @v0.83.0, whose `ExtensionShortcut`
    /// carries `{shortcut, description?, handler, extensionPath}` at `:1524-1529`). The host
    /// invokes [`NativeExtension::execute_shortcut`] when the key fires. `description` is what
    /// `/hotkeys` renders in the Action column — upstream's `const description =
    /// shortcut.description ?? shortcut.extensionPath;`
    /// (`modes/interactive/interactive-mode.ts:5856`).
    pub fn register_shortcut(&mut self, key: impl Into<String>, description: Option<String>) {
        self.shortcuts.push((key.into(), description));
    }

    /// Register this extension's markdown transformer (EXT-019; pi
    /// `registerMarkdownTransformer(transformer)`, `extensions/types.ts:1292` @v0.84.1, impl
    /// `loader.ts:309-312`). The transformer itself is [`NativeExtension::transform_markdown`];
    /// this only declares that it exists, matching the guest side, where the closure lives behind
    /// the `transform-markdown` export. Calling it twice is the same as calling it once — upstream
    /// ASSIGNS the field, so an extension has at most one transformer.
    pub fn register_markdown_transformer(&mut self) {
        self.markdown_transformer = true;
    }

    /// Subscribe to raw terminal input (EXT-021; pi
    /// `ExtensionUIContext.onTerminalInput(handler)`, `extensions/types.ts:145` @v0.83.0). The
    /// handler itself is [`NativeExtension::on_terminal_input`]; this only declares that it
    /// exists, matching the guest side where the closure lives behind the `on-terminal-input`
    /// export.
    pub fn subscribe_terminal_input(&mut self) {
        self.terminal_input = true;
    }

    /// Declare a CLI flag (EXT-035; pi `registerFlag`, `extensions/loader.ts:274-410` @v0.83.0).
    /// `spec` is the flag's JSON spec; the resolved value is read back through
    /// [`crate::ExtensionRegistry::flag`].
    pub fn register_flag(&mut self, name: impl Into<String>, spec: serde_json::Value) {
        self.flags.push((name.into(), spec));
    }

    /// Contribute a custom provider (EXT-035; pi `registerProvider`). `config` is the
    /// [`crate::ProviderConfig`] shape as JSON, matching the guest's
    /// `registration.register-provider` import so the two tiers register identically.
    pub fn register_provider(&mut self, id: impl Into<String>, config: serde_json::Value) {
        self.providers.push((id.into(), config));
    }

    /// Opt a registered command into argument autocomplete (EXT-035; the native analog of the
    /// guest's `registration.add-autocomplete` import).
    ///
    /// [CYRUP-DELTA] EXT-062: pi has no `addAutocomplete` CALL. Upstream this is a FIELD on the
    /// command's own options bag — `getArgumentCompletions?: (argumentPrefix: string) =>
    /// AutocompleteItem[] | null | Promise<…>` on `RegisteredCommand`
    /// (`extensions/types.ts:1166` @v0.83.0) — passed inline to `registerCommand`. A WIT record
    /// cannot carry a closure, so the closure inverts into a flag plus an export; the native tier
    /// keeps the same shape as the WASM tier so the two do not diverge from each other. Same
    /// inversion as `tool-descriptor.prepare-arguments` and `.has-renderer`.
    pub fn add_autocomplete(&mut self, command: impl Into<String>) {
        self.autocomplete.push(command.into());
    }

    /// Stack one global autocomplete provider (EXT-035; pi `addAutocompleteProvider`,
    /// `extensions/types.ts:225` @v0.83.0 — EXT-072 cluster A: the `:218` this cited is
    /// `getEditorText`'s doc line).
    pub fn add_autocomplete_provider(&mut self) {
        self.autocomplete_providers += 1;
    }

    /// Listen on an inter-extension bus topic (EXT-018; pi `pi.events.on(channel, handler)`,
    /// `core/event-bus.ts:18`). Deliveries arrive at [`NativeExtension::on_bus_event`].
    pub fn subscribe_bus(&mut self, topic: impl Into<String>) {
        self.bus_topics.push(topic.into());
    }

    pub fn subscriptions(&self) -> Subscriptions {
        self.subs
    }

    pub(crate) fn into_parts(self) -> InitParts {
        (
            self.subs,
            self.tools,
            self.commands,
            self.tool_renderers,
            self.message_renderers,
            self.entry_renderers,
            self.shortcuts,
            self.flags,
            self.providers,
            self.autocomplete,
            self.autocomplete_providers,
            self.bus_topics,
            self.markdown_transformer,
            self.terminal_input,
        )
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

    /// Receive an inter-extension bus event this extension subscribed to (EXT-018).
    ///
    /// pi hangs ONE `createEventBus()` on the ONE `ExtensionAPI` object it builds for every
    /// extension it loads — `events: eventBus,`
    /// (`pi/packages/coding-agent/src/core/extensions/loader.ts:389` @v0.83.0, impl
    /// `core/event-bus.ts:12-32`) — and upstream has a single extension kind, so "every extension
    /// gets the bus" needs no qualification. cyrup's bus lived inside the `wasm-host` feature gate
    /// and resolved subscribers out of the LIVE WASM map only, which meant the three extensions
    /// cyrup actually ships (permission-system, intercom, subagents — all natives) had no
    /// `pi.events` at all and had to re-invent cross-extension coordination out of band.
    ///
    /// Subscribe by declaring the topic through [`InitApi::subscribe_bus`] during
    /// [`Self::init`]; the host then calls this with an EVENT-tier ctx (a bus listener is not a
    /// command, so session-replacement ops stay refused, matching the tier every other handler
    /// runs at). Default: ignore, so a built-in that does not use the bus needs no code.
    ///
    /// An `Err` is CONTAINED, logged, and surfaced on the `onError` channel (EXT-057) — it never
    /// stops the rest of the fan-out, matching pi's per-listener `catch`.
    async fn on_bus_event(
        &self,
        _topic: &str,
        _payload: &serde_json::Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        Ok(())
    }

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

    /// Whether this built-in is **ambient** — present because it is installed, not because the
    /// embedder named it — and therefore switched off by `--no-extensions` (SEAM-071).
    ///
    /// pi splits the extension set in exactly this way and gates only one half. `noExtensions`
    /// reduces the PATH tier to the explicit `-e` paths
    /// (`const extensionPaths = this.noExtensions ? cliEnabledExtensions : this.mergePaths(...)`,
    /// `resource-loader.ts:451-452` @v0.83.0), which is where installed packages like
    /// `@gotgenes/pi-permission-system` and pi-intercom live. The INLINE tier is untouched:
    /// `loadFinalExtensionSet` calls `loadExtensionFactories(...)` unconditionally (`:579-581`) over
    /// `extensionFactories = [...builtInExtensions, ...(options?.extensionFactories ?? [])]`
    /// (`main.ts:523`) — pi's own `llama.cpp` provider extension plus whatever an embedder passed
    /// programmatically. An inline factory the caller handed in by value is not something a flag
    /// about *discovery* can be about.
    ///
    /// `false` is therefore the right default: [`crate::ExtensionHost::load_native`]'s caller passed
    /// this object by hand, which is pi's inline-factory tier. A built-in that stands in for an
    /// upstream INSTALLED PACKAGE — cyrup compiles in what pi installs — must override this to
    /// `true` so `--no-extensions` means the same thing in both products.
    fn is_ambient(&self) -> bool {
        false
    }

    /// Execute a registered slash command this extension owns (Pi `command.handler(args, ctx)`,
    /// agent-session.ts:1159; R-08-016). `ctx` is **command-tier** (session mutation allowed). The
    /// optional `String` is the command's text output (Pi commands return `void`; cyrup mirrors the
    /// WASM `execute-command` shape so the two paths are interchangeable). The default rejects: a
    /// native built-in that registers a command via [`InitApi::register_command`] MUST override this
    /// to service it. Built-ins that only subscribe to events leave it unimplemented.
    ///
    /// # How the return value reaches the user — and the `Ok(None)` convention
    ///
    /// **`Ok(Some(text))` is surfaced by the session as an [`crate::NotifyKind::Info`]
    /// notification** (`cyrup-session-svc/src/session.rs`, the `Ok(Some(_))` arm of
    /// `try_execute_extension_command`). Trimmed-empty text surfaces nothing. An `Err` surfaces as
    /// an [`crate::NotifyKind::Error`] notification prefixed `command:<name>: `, mirroring Pi's
    /// `emitError({ extensionPath: \`command:${commandName}\`, … })`
    /// (agent-session.ts:1295-1299); either way the command counts as HANDLED and the `/name …`
    /// text never reaches the model as a prompt (Pi `return true`, :1292 and :1300).
    ///
    /// This return channel cannot carry a notification LEVEL — it is a `String`, and everything on
    /// it arrives as Info. So:
    ///
    /// - A handler that just wants to say something returns `Ok(Some(text))` and gets Info. This is
    ///   the common case and needs no thought.
    /// - **A handler that needs `Warning` or `Error` calls
    ///   [`crate::HostServices::notify`] itself with the level it wants, and then returns
    ///   `Ok(None)`.** Returning both the notification AND the text would put the same message on
    ///   screen twice, once at the chosen level and once as an Info duplicate.
    ///
    /// `Ok(None)` therefore means "nothing further to surface" — either the handler genuinely has
    /// no output, or it has already surfaced its own at a level this channel cannot express. Both
    /// are silent here, which is correct in both cases.
    ///
    /// Reserve `Err` for the command genuinely FAILING (bad routing, a panic, an unserviceable
    /// name). A user-facing error the handler expects and wants to phrase itself is better sent as
    /// a self-issued `Error` notify plus `Ok(None)`, which keeps the wording under the handler's
    /// control instead of wrapping it in the `command:<name>: ` prefix.
    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        Err(ExtError::Component(format!("native extension has no handler for command `{name}`")))
    }

    /// Run the keyboard shortcut declared through [`InitApi::register_shortcut`] (EXT-035).
    ///
    /// pi's shortcut handler is `handler: (ctx: ExtensionContext) => Promise<void> | void`
    /// (`pi/packages/coding-agent/src/core/extensions/types.ts:1249-1255` @v0.83.0, the same shape
    /// on `ExtensionShortcut` at `:1524-1529`) — it returns nothing, so there is no output channel
    /// to mirror here: a handler with something to say calls [`crate::HostServices::notify`], as it
    /// does upstream through `ctx.ui.notify`.
    ///
    /// `ctx` is COMMAND tier, matching the guest path
    /// ([`crate::host::LiveExtension::execute_shortcut`]) and pi, where a shortcut handler receives
    /// the same `ExtensionContext` a command handler does and may therefore call session-replacing
    /// ops.
    ///
    /// Without this, `InitApi::register_shortcut` was a write-only surface: the key landed in the
    /// registry, `shortcut_keys()` advertised it, `/hotkeys` listed it, and pressing it resolved an
    /// owner that `run_shortcut` could not reach because it looked only in the live-WASM map.
    async fn execute_shortcut(&self, key: &str, _ctx: &HostCtx) -> Result<(), ExtError> {
        Err(ExtError::Component(format!("native extension has no handler for shortcut `{key}`")))
    }

    /// Render a tool CALL / custom MESSAGE this extension declared a renderer for (Pi
    /// `renderCall`, extensions/types.ts:489). `key` is the TOOL NAME for a tool renderer
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
    /// extensions/types.ts:492-497).
    fn render_result(&self, _key: &str, _result: &serde_json::Value) -> Option<serde_json::Value> {
        None
    }

    /// Transform transcript markdown before the host renders it (EXT-019; pi
    /// `MarkdownTransformer = (markdown, context) => string`, `extensions/types.ts:1153` @v0.84.1
    /// — a POST-BASELINE addition, absent at v0.83.0). Called only when [`Self::init`] declared one
    /// through [`InitApi::register_markdown_transformer`].
    ///
    /// `ctx` is `MarkdownTransformContext` (`types.ts:1147-1151`):
    /// `{messageType: "user"|"assistant"|"assistant-thinking", isStreaming, availableWidth}`.
    ///
    /// Sync for the same reason as [`Self::render_call`]: it runs on the UI's render path. A PANIC
    /// is contained by the host and the text passes through unchanged, so a broken transformer can
    /// never blank a line of transcript.
    fn transform_markdown(&self, markdown: &str, _ctx: &serde_json::Value) -> String {
        markdown.to_string()
    }

    /// Inspect one raw terminal-input chunk (EXT-021; pi `TerminalInputHandler`,
    /// `extensions/types.ts:113` @v0.83.0: `(data: string) => {consume?, data?} | undefined`).
    /// Only consulted on a native that declared [`InitApi::subscribe_terminal_input`].
    ///
    /// `None` is upstream's `undefined` — "I looked at it and did nothing". Sync for the same
    /// reason as [`Self::render_call`]: it runs on the UI's input path. A PANIC is contained by
    /// the host and treated as `None`, so a broken extension can never swallow the keyboard.
    fn on_terminal_input(&self, _data: &str) -> Option<crate::TerminalInputResult> {
        None
    }

    /// Render a custom ENTRY this extension declared a renderer for via
    /// [`InitApi::register_entry_renderer`] (Pi `EntryRenderer`, extensions/types.ts:1165-1169).
    /// `custom_type` is the entry's `customType`; `entry` is the serialized session entry.
    ///
    /// `None` — the upstream `Component | undefined` return — means "I chose to draw nothing"; the
    /// host then draws nothing at all, matching `CustomEntryComponent.hasContent() === false`
    /// (`interactive-mode.ts:3438-3440`). A PANIC is the `throw` of `custom-entry.ts:47`: the host
    /// contains it and reports [`crate::RenderOutcome::Failed`], which draws the failure box.
    ///
    /// Sync for the same reason as [`Self::render_call`]: it runs on the UI's event path.
    fn render_entry(
        &self,
        _custom_type: &str,
        _entry: &serde_json::Value,
    ) -> Option<serde_json::Value> {
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
    /// The live source of the rich ctx fields, when the host was given one
    /// ([`crate::ExtensionHost::set_ctx_source`], which
    /// [`crate::ExtensionHost::load_native_with_services`] feeds from the injected
    /// `HostServices`). Used to refresh [`HostCtxRich`] on EVERY dispatch (EXT-005):
    /// idle/trust/usage/model/system-prompt are all live values, so a ctx built once at load time
    /// would go stale — and, before EXT-005, `HostCtxRich::default()` meant a native built-in read a
    /// confident `is_idle = false` / `is_project_trusted = false` rather than the truth.
    ///
    /// EXT-060: NOT `wasm-host`-gated. It was, because it was typed against
    /// [`crate::host::HostServices`], which lives behind that feature — so a
    /// `--no-default-features` build (the manifest explicitly invites one: "the native-builtin
    /// dispatch foundation… builds without pulling Wasmtime") kept the whole EXT-005 fix out and
    /// silently handed every native built-in `is_idle = false` / `is_project_trusted = false`,
    /// with no diagnostic. pi has one extension kind and one `ExtensionAPI`
    /// (`extensions/loader.ts:274-410` @v0.83.0) whose `ExtensionContext` data fields
    /// (`extensions/types.ts:329-346`) are populated unconditionally; which host features are
    /// compiled in is not something a handler's `ctx.isIdle` is allowed to depend on.
    ctx_source: Option<Arc<dyn HostCtxSource>>,
}

/// The live source of pi's `ExtensionContext` DATA fields (`extensions/types.ts:329-346`) for a
/// native dispatch — model, idle, project-trust, context usage, system prompt.
///
/// CYRUP-DELTA: upstream needs no such trait; `ExtensionContext` is one object built by one loader
/// (`extensions/loader.ts:274-410` @v0.83.0). cyrup needs a feature-independent seam because the
/// full capability backend ([`crate::host::HostServices`]) only exists with the Wasmtime host
/// compiled in, while native built-ins — and their `ctx.is_idle()` / `ctx.is_project_trusted()`
/// reads — exist on both arms. This is the narrow read-only slice of that backend, so the arms can
/// stop disagreeing: with `wasm-host` the host wires it straight off the injected `HostServices`
/// (blanket impl below); without it, a host embedder attaches its own via
/// [`crate::ExtensionHost::set_ctx_source`].
pub trait HostCtxSource: Send + Sync {
    /// Snapshot the rich fields as of RIGHT NOW (re-read per dispatch, never cached).
    fn rich(&self) -> HostCtxRich;
}

/// Adapts the injected [`crate::host::HostServices`] backend to [`HostCtxSource`] — the live rich
/// values are exactly the five getters EXT-005 reads. A wrapper rather than a blanket impl because
/// `Arc<dyn HostServices>` cannot be coerced to `Arc<dyn HostCtxSource>` through one.
#[cfg(feature = "wasm-host")]
pub struct ServicesCtxSource(pub Arc<dyn crate::host::HostServices>);

#[cfg(feature = "wasm-host")]
impl HostCtxSource for ServicesCtxSource {
    fn rich(&self) -> HostCtxRich {
        rich_from_services(self.0.as_ref())
    }
}

impl NativeHandle {
    pub fn new(
        inner: Arc<dyn NativeExtension>,
        subs: Subscriptions,
        ctx: HostCtx,
    ) -> Self {
        let id = inner.id();
        Self { id, subs, ctx, inner, ctx_source: None }
    }

    /// Attach the live rich-ctx source so each dispatch gets a FRESH [`HostCtxRich`] (EXT-005).
    #[must_use]
    pub fn with_ctx_source(mut self, source: Option<Arc<dyn HostCtxSource>>) -> Self {
        self.ctx_source = source;
        self
    }

    /// The ctx for one dispatch: the handle's stable base ctx (tier, mode, cwd and — critically —
    /// the SHARED [`HumanWaitGate`] the dispatcher's budget watchdog polls) with the rich fields
    /// re-read from the live backend.
    fn dispatch_ctx(&self) -> HostCtx {
        match &self.ctx_source {
            Some(src) => self.ctx.clone().with_rich(src.rich()),
            None => self.ctx.clone(),
        }
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
        // EXT-061: the native tier reads the SAME backend accessor the WIT import does, so a
        // built-in and a guest cannot disagree about the bag.
        system_prompt_options: svc.system_prompt_options(),
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

    /// A native's subscription set is fixed by [`InitApi::subscribe`] during `init` — there is no
    /// native equivalent of the guest's late `subscribe` import — so this is the stored bitset.
    /// Returned by value per [`Extension::subscriptions`] (EXT-058).
    fn subscriptions(&self) -> Subscriptions {
        self.subs
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

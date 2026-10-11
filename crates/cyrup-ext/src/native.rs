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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A live component a NATIVE renderer handed back — Pi's `Component`
/// (`packages/tui/src/index.ts`), whose `render(width)` the host calls on EVERY frame.
///
/// Native-only by construction: a component cannot cross the WIT boundary to a WASM guest, and does
/// not need to — a compiled-in built-in already gets its own hooks for exactly this reason.
pub trait RenderedComponent: std::fmt::Debug + Send + Sync {
    /// Pi `Component.render(width): string[]`. Returns one string per display row.
    ///
    /// The caller is free to invoke this on EVERY frame, so it must be cheap and MUST NOT panic;
    /// a panic is contained by the host and drawn as the renderer-failed box.
    fn render(&self, ctx: &RenderCtx<'_>) -> Vec<String>;
}

/// The render context Pi passes as `(width, options, theme)` (`MessageRenderer`,
/// `extensions/types.ts:1284`). The typed counterpart of the JSON `MarkdownTransformContext` bag,
/// which already carries `availableWidth`; typed here because a component is native-only.
pub struct RenderCtx<'a> {
    /// The live terminal content width, in display columns.
    pub width: usize,
    /// Pi `options.expanded` — the live expand-toggle flag, resolved per frame and NEVER frozen.
    pub expanded: bool,
    /// Pi `theme`. Read on every render, so a live theme change repaints without invalidating
    /// anything the component has cached.
    pub theme: &'a dyn RenderTheme,
}

/// Pi's `Theme`, narrowed to what a renderer actually calls. Implemented host-side over the live
/// palette.
pub trait RenderTheme: Send + Sync {
    /// Pi `theme.fg(color, text)` — wrap `text` in the styling for the named role (`muted`,
    /// `toolTitle`, `text`, `dim`, `accent`, `error`, …). An unknown role returns `text` unchanged;
    /// upstream's `Theme.fg` never throws.
    fn fg(&self, role: &str, text: &str) -> String;
    /// Pi `theme.bold(text)`.
    fn bold(&self, text: &str) -> String;

    /// Pi `keyHint(keybinding, description)` (`keybinding-hints.ts`): the binding's CURRENT key text
    /// in `dim`, then ` description` in `muted`. `binding` is pi's keybinding id
    /// (`"app.tools.expand"`). The default has no keymap to resolve against and shows the id.
    fn key_hint(&self, binding: &str, description: &str) -> String {
        format!(
            "{}{}",
            self.fg("dim", binding),
            self.fg("muted", &format!(" {description}"))
        )
    }

    /// Pi `highlightCode(code, lang)` (`theme.ts`): one SGR-styled string per source line. The
    /// default is the unstyled lines, which is pi's own fallback when a language has no grammar.
    fn highlight_code(&self, code: &str, _lang: &str) -> Vec<String> {
        code.split('\n').map(str::to_string).collect()
    }

    /// The per-block transform of pi's `getTextOutput` (`render-utils.ts`):
    /// `sanitizeBinaryOutput(stripAnsi(text)).replace(/\r/g, "")`. The default is the identity.
    fn display_text(&self, text: &str) -> String {
        text.to_string()
    }
}

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

/// What a handler is doing while it holds a [`SanctionedWaitGuard`] — the reason it is allowed to
/// outlive the dispatch budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SanctionedWaitKind {
    /// A human answering a dialog (P-3 — the permission gate's `ask`, the MCP approval prompt).
    /// Unbounded by the gate: the handler's own timeout (which fails CLOSED) is the bound.
    Human,
    /// A model call the handler must await before it may return — the watchdog's agent-end review,
    /// which upstream awaits inside its `agent_end` handler (`register-main.ts:427-430`,
    /// `register-child.ts:102-110` @v0.43.0) with no per-handler budget at all
    /// (`coding-agent/src/core/extensions/runner.ts:805-811`). Always declared with a ceiling.
    ModelReview,
    /// A headless session finishing its outstanding background work before the process exits — the
    /// subagents extension's auto-drain, which upstream AWAITS inside its `agent_end` handler
    /// (`pi-subagents` `extension/index.ts:834`, child `runs/shared/subagent-prompt-runtime.ts:500`
    /// @v0.68.0) bounded only by the drain's own `timeoutMs` (`runs/background/auto-drain.ts:7,48`).
    /// Cutting it at the dispatch budget loses exactly the completions the drain exists to deliver.
    /// Always declared with a ceiling derived from that `timeoutMs`.
    AutoDrain,
    /// A `tool_call` handler holding the call until the servers it needs have connected — the MCP
    /// extension's wait before a `codemode` script or a `tool_search` runs, so the tools of a server
    /// that is still starting are registered when it looks (`pi.on("tool_call")` in
    /// `extensions/mcp/index.ts` @v1.0.4, which awaits the servers' `ready` promises). Always
    /// declared with a ceiling; the handler gives up and lets the call run before it passes.
    ServerConnect,
}

/// One live sanctioned wait.
#[derive(Debug)]
struct LiveWait {
    id: u64,
    kind: SanctionedWaitKind,
    /// `None` for an unbounded (human) wait; otherwise the instant past which the dispatcher stops
    /// forgiving this wait.
    until: Option<tokio::time::Instant>,
}

/// Coordinates a SANCTIONED LONG WAIT between a native `on_event` handler and the dispatch
/// invocation budget.
///
/// # The problem
///
/// The native dispatcher wraps every handler in a [`crate::dispatch::DEFAULT_INVOKE_BUDGET`] (5 s)
/// deadline and, on expiry, DROPS the handler future (`dispatch.rs`, `invoke_contained`). That is
/// cyrup's hang protection and upstream has none (pi's runner simply `await`s each handler,
/// `runner.ts:805-811`). Three kinds of handler legitimately take longer:
///
/// - **a human** (P-3, `spec/extensions/cyrup-permission-system-port.md §4`): a permission gate's
///   `before_tool_call` `ask`. Dropping it at the budget is **fail-OPEN** — the tool runs ungated.
/// - **a model review** (UW-3): the watchdog's agent-end review is AWAITED inside the `AgentEnd`
///   handler, bounded by its own `agentEndTimeoutMs` (30 s default). Dropping it at 5 s loses the
///   review, every warning it would have raised, and — in an armed subagent child — the terminal
///   `idle`/`failed`/`stale` status the parent is waiting for, leaving the parent's view stuck at
///   `reviewing`.
/// - **a headless auto-drain** ([`SanctionedWaitKind::AutoDrain`]): the subagents extension's
///   `AgentEnd` handler waits for the session's background runs to land before a `-p` process
///   exits, bounded by the drain's own timeout. Dropping it at 5 s abandons every completion that
///   lands after that — the whole reason the drain exists.
///
/// # The shape
///
/// A handler DECLARES the wait by holding a [`SanctionedWaitGuard`] across it
/// ([`HostCtx::begin_human_wait`], [`HostCtx::begin_sanctioned_wait`]). While any guard it holds is
/// live the dispatcher's budget watchdog re-arms instead of firing — it never suspends the handler
/// future itself, so the very call that will drop the guard keeps running. Every handler that
/// declares nothing keeps the exact fail-fast budget: a cooperative runaway is still cut at 5 s.
///
/// A guard may carry a CEILING ([`HostCtx::begin_sanctioned_wait`]): the dispatcher forgives it
/// only until the ceiling, so a declared-bounded wait whose own timeout is broken is still cut
/// (at the first budget boundary past the ceiling). A human wait has no ceiling — a human's latency
/// is not the handler's to bound, and the permission gate's own timeout fails closed.
///
/// [CYRUP-DELTA] Upstream has no per-handler budget to extend. This keeps the protection cyrup
/// added and fixes the one thing it breaks — work the handler is REQUIRED to await — rather than
/// exempting a whole event kind (which would drop the protection for every extension) or moving
/// the review off the handler (which would reorder settle after review, `register-child.ts:102-110`).
///
/// Reentrant: nested or overlapping guards are tracked individually; the budget resumes once none
/// of them forgives.
#[derive(Debug, Default)]
pub struct SanctionedWaitGate {
    next_id: AtomicU64,
    waits: std::sync::Mutex<Vec<LiveWait>>,
}

/// The gate's original name, from when a human was the only sanctioned wait (P-3). Kept as an alias
/// because the permission gate and the MCP approval path name it.
pub type HumanWaitGate = SanctionedWaitGate;

impl SanctionedWaitGate {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<LiveWait>> {
        self.waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// True while at least one live guard still forgives the budget right now: an unbounded
    /// (human) wait, or a bounded one whose ceiling has not passed.
    pub fn is_waiting(&self) -> bool {
        let now = tokio::time::Instant::now();
        self.lock()
            .iter()
            .any(|wait| wait.until.is_none_or(|until| now < until))
    }

    /// The kinds of every live guard (including any past its ceiling), in the order they began.
    pub fn live_kinds(&self) -> Vec<SanctionedWaitKind> {
        self.lock().iter().map(|wait| wait.kind).collect()
    }

    fn begin(
        self: &Arc<Self>,
        kind: SanctionedWaitKind,
        ceiling: Option<std::time::Duration>,
    ) -> SanctionedWaitGuard {
        let id = self.next_id.fetch_add(1, Ordering::AcqRel);
        let until = ceiling.map(|ceiling| tokio::time::Instant::now() + ceiling);
        self.lock().push(LiveWait { id, kind, until });
        SanctionedWaitGuard {
            gate: Arc::clone(self),
            id,
        }
    }
}

/// RAII guard for a sanctioned long wait (see [`SanctionedWaitGate`]). While held, the dispatch
/// budget is extended (up to the guard's ceiling, if it has one); on drop (including during a panic
/// unwind) it is removed, so the handler's post-wait wrap-up is budgeted again.
#[must_use = "the dispatch budget is only extended while the guard is held"]
pub struct SanctionedWaitGuard {
    gate: Arc<SanctionedWaitGate>,
    id: u64,
}

/// The guard's original name (P-3). See [`HumanWaitGate`].
pub type HumanWaitGuard = SanctionedWaitGuard;

impl std::fmt::Debug for SanctionedWaitGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SanctionedWaitGuard")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for SanctionedWaitGuard {
    fn drop(&mut self) {
        let id = self.id;
        self.gate.lock().retain(|wait| wait.id != id);
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
    /// The sanctioned-long-wait coordinator (P-3, UW-3). Shared (via `Arc`) with the dispatcher's
    /// budget watchdog through [`Extension::human_wait_gate`], so a handler's
    /// [`Self::begin_human_wait`] / [`Self::begin_sanctioned_wait`] and the watchdog consult the SAME
    /// gate. One per handler ctx.
    human_wait: Arc<HumanWaitGate>,
    /// The tool call that made the call this event is about, when a tool is calling a tool
    /// (CODE-006; pi's `parentToolCallId` on the `tool_call` / `tool_result` / `tool_execution_*`
    /// events, `extensions/types.ts:1061-1083`, `:1155-1161`, `:1231` @v1.0.1). `None` for every
    /// event the agent loop itself raised.
    parent_tool_call_id: Option<cyrup_core::ToolCallId>,
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
            parent_tool_call_id: None,
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
            parent_tool_call_id: None,
        }
    }

    /// Attach the rich native-ctx fields (Pi `ExtensionContext`, gap-08 #6).
    #[must_use]
    pub fn with_rich(mut self, rich: HostCtxRich) -> Self {
        self.rich = rich;
        self
    }

    /// The rich native-ctx fields (model/idle/trust/usage/system-prompt).
    pub fn rich(&self) -> &HostCtxRich {
        &self.rich
    }

    /// Mark this ctx as the one for an event of a call another tool made (CODE-006).
    #[must_use]
    pub fn with_parent_tool_call_id(mut self, parent: cyrup_core::ToolCallId) -> Self {
        self.parent_tool_call_id = Some(parent);
        self
    }

    /// The tool call that made the call this event is about — pi's `event.parentToolCallId`
    /// (`extensions/types.ts:1061-1083`, `:1155-1161`, `:1231` @v1.0.1). `Some` only for the
    /// `tool_call`, `tool_result` and `tool_execution_*` events of a call a tool made while it ran
    /// (`ctx.executeTool`), and then it is the id of the calling tool call — which is itself
    /// `<id>/<n>` when that tool was a nested call. `None` for a model-issued call.
    ///
    /// A handler that gates on `tool_call` needs no change to cover nested calls: they reach it
    /// as the same event. This is for the handler that must tell them apart — one that records
    /// only what the model asked for, for instance.
    pub fn parent_tool_call_id(&self) -> Option<&cyrup_core::ToolCallId> {
        self.parent_tool_call_id.as_ref()
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
        Ok(self.rich.system_prompt_options.clone().unwrap_or_else(
            || serde_json::json!({ "cwd": self.cwd.to_string_lossy().into_owned() }),
        ))
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
        self.human_wait.begin(SanctionedWaitKind::Human, None)
    }

    /// Declare a sanctioned long wait of `kind` that must not outlive `ceiling` (UW-3): hold the
    /// returned guard across work the handler is REQUIRED to await — the watchdog's agent-end model
    /// review — and the dispatcher extends this handler's budget until the guard drops or the
    /// ceiling passes, whichever is first. Every other handler keeps the fail-fast budget.
    ///
    /// `ceiling` should be the wait's own bound plus the handler's remaining work: a guard whose
    /// ceiling is shorter than the work it covers is cut exactly as an undeclared handler is.
    #[must_use = "hold the guard across the long wait; dropping it immediately does nothing"]
    pub fn begin_sanctioned_wait(
        &self,
        kind: SanctionedWaitKind,
        ceiling: std::time::Duration,
    ) -> SanctionedWaitGuard {
        self.human_wait.begin(kind, Some(ceiling))
    }

    /// The shared [`HumanWaitGate`] backing [`Self::begin_human_wait`] (P-3). The dispatcher reads this
    /// (via [`Extension::human_wait_gate`]) so its budget watchdog consults the SAME gate the handler
    /// signals through.
    pub fn human_wait_gate(&self) -> Arc<HumanWaitGate> {
        Arc::clone(&self.human_wait)
    }

    /// Deadlock guard (R-08-008): returns `Err(ExtError::Deadlock)` if a session-replacement
    /// control op (new-session/switch/fork/navigate/reload/compact/wait-idle) is attempted from an
    /// event handler. Authoritative regardless of the guest SDK's types.
    ///
    /// EXEMPT, and live.rs does not consult this gate for any of them:
    /// * GAP-11 — `set_model`/`set_thinking_level`; pi allows them from any handler
    ///   (`loader.ts:342-354`).
    /// * EXT-087 — `send-message`/`send-user-message`; upstream's bodies carry no tier check at all
    ///   (`core/extensions/loader.ts:351-354`, `:356-358` @v0.87.1). They queue unconditionally and
    ///   apply at the post-settle drain in `cyrup-session-svc`'s `AgentSession::settle_run`.
    ///
    /// EXT-087, recorded because the row expected otherwise: there is no NATIVE send path to
    /// exempt. This list used to name the two send ops, which read as though a native extension
    /// were gated on them somewhere — it is not. `ControlOp::SendMessage`/`SendUserMessage` have
    /// exactly ONE producer in the workspace, `host/live.rs`'s two wasm imports; a native extension
    /// queues through `HostServices::control` directly, which has never consulted a tier. So the
    /// wasm and native paths agree BECAUSE this gate is gone from the wasm side, not because a
    /// matching native gate was also removed — and the EXT-054/-055 divergence class the row was
    /// worried about does not arise here.
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
    /// LIVE providers ([`Self::register_provider_live`]): the extension's own
    /// `Arc<dyn Provider>`, kept apart from the JSON `providers` because it cannot ride in
    /// [`InitParts`]' `serde_json::Value`. Taken by the facade with
    /// [`Self::take_live_providers`] before [`Self::into_parts`].
    live_providers: Vec<(String, Arc<dyn cyrup_provider::Provider>)>,
    /// VIRTUAL models ([`Self::register_virtual_model`]): pi's `pi.registerVirtualModel()` at
    /// extension-load time, which upstream pushes onto `pendingVirtualModelRegistrations`
    /// (`extensions/loader.ts:226-228` @v1.0.4). Kept apart from `providers` for the same reason
    /// `live_providers` is: a `VirtualModelDefinition` carries an `Arc<dyn ModelRouter>` and cannot
    /// ride in [`InitParts`]' `serde_json::Value`. Taken by the facade with
    /// [`Self::take_virtual_models`] before [`Self::into_parts`].
    virtual_models: Vec<cyrup_provider::VirtualModelDefinition>,
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
    /// EXT-064: whether this extension subscribed to git-branch changes. A BOOL for the same
    /// reason as `terminal_input`: a native has exactly one
    /// [`NativeExtension::on_branch_change`].
    branch_change: bool,
    /// Typed-bus topics ([`Self::subscribe_typed_bus`]), delivered synchronously to
    /// [`NativeExtension::on_typed_bus_event`]. Taken by the facade with
    /// [`Self::take_typed_bus_topics`] before [`Self::into_parts`].
    typed_bus_topics: Vec<String>,
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
    ///
    /// While the tool's `execute` runs inside a session, [`crate::ExtensionToolContext::current`]
    /// answers pi's `ctx.executeTool` / `ctx.tools` bound to that call (`extensions/types.ts:367-395`
    /// @v1.0.1) — see [`crate::nested`].
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

    /// Subscribe to git-branch changes (EXT-064; pi
    /// `ReadonlyFooterDataProvider.onBranchChange(callback)`,
    /// `core/footer-data-provider.ts:139-143` @v0.87.1). The handler itself is
    /// [`NativeExtension::on_branch_change`]; this only declares that it exists, matching the guest
    /// side where the callback lives behind the `on-branch-change` export.
    pub fn subscribe_branch_change(&mut self) {
        self.branch_change = true;
    }

    /// Declare a CLI flag (EXT-035; pi `registerFlag`, `extensions/loader.ts:274-410` @v0.83.0).
    /// `spec` is the flag's JSON spec; the resolved value is read back through
    /// [`crate::ExtensionRegistry::flag_value`].
    pub fn register_flag(&mut self, name: impl Into<String>, spec: serde_json::Value) {
        self.flags.push((name.into(), spec));
    }

    /// Contribute a custom provider (EXT-035; pi `registerProvider`). `config` is the
    /// [`crate::ProviderConfig`] shape as JSON, matching the guest's
    /// `registration.register-provider` import so the two tiers register identically.
    pub fn register_provider(&mut self, id: impl Into<String>, config: serde_json::Value) {
        self.providers.push((id.into(), config));
    }

    /// Contribute a LIVE provider: the extension's own `Arc<dyn cyrup_provider::Provider>`, stored
    /// by the registry as given instead of being rebuilt from a JSON config into a
    /// `ConfigProvider`. pi's `registerProvider(provider: Provider)` overload takes exactly that — a
    /// finished provider object the `ModelRegistry` holds and calls (`extensions/types.ts:1803`
    /// @v0.99.2-17; the `(name, config)` form is `:1804`).
    ///
    /// `id` must equal `provider.id()`; a mismatch fails the extension's load. Registering the same
    /// id again, from here or later through [`LateRegistrar::register_provider_live`], REPLACES the
    /// provider — pi's "replaces all models" (`model-registry.ts:919` @v0.84.1) — and the model lists
    /// see the replacement's catalog.
    pub fn register_provider_live(
        &mut self,
        id: impl Into<String>,
        provider: Arc<dyn cyrup_provider::Provider>,
    ) {
        self.live_providers.push((id.into(), provider));
    }

    /// Register a VIRTUAL model: a selectable catalog entry that routes each request to a physical
    /// model — pi `pi.registerVirtualModel(model)` (`extensions/types.ts:1865-1873` @v1.0.4).
    ///
    /// The selection (`ctx.model`, the branch's `model_change` entries) names the VIRTUAL model;
    /// assistant messages record the physical model and thinking level the router picked. `provider`
    /// may be any provider id, including one that already has physical models, and may list several
    /// virtual models. Registering the same `(provider, id)` again REPLACES the virtual model, in
    /// place — pi's `Map.set` keeps the key's position and the position is observable in `/model`.
    ///
    /// At `init` time this QUEUES, exactly as upstream's pre-bind
    /// `pendingVirtualModelRegistrations` does (`extensions/loader.ts:226-228`): the model registry
    /// does not exist yet. The facade hands the queue to the registry after `init` returns, and
    /// [`crate::registry::ExtensionRegistry::bind_model_registry`] flushes it into the session's
    /// registry. Post-`init`, use [`LateRegistrar::register_virtual_model`].
    pub fn register_virtual_model(&mut self, definition: cyrup_provider::VirtualModelDefinition) {
        self.virtual_models.push(definition);
    }

    /// The virtual models declared through [`Self::register_virtual_model`], moved out.
    pub(crate) fn take_virtual_models(&mut self) -> Vec<cyrup_provider::VirtualModelDefinition> {
        std::mem::take(&mut self.virtual_models)
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

    /// Listen on a TYPED bus topic: an event another native emits through
    /// [`crate::SharedBus::emit_typed`] (a native reaches it with
    /// [`crate::host::HostServices::emit_typed_event`]) is handed to
    /// [`NativeExtension::on_typed_bus_event`] inline, before the emit returns.
    ///
    /// This is the half of pi's `pi.events` the JSON topics of [`Self::subscribe_bus`] do not
    /// carry: an emit whose payload is a typed request the listeners answer synchronously, and whose
    /// emitter reads the answer right after `emit` — pi-intercom's `IntercomSessionIdentityRequestV1
    /// { version: 1; claim(stableId) }` (`v0.14.0 extension-api.ts:17-20`, emitted and read at
    /// `index.ts:1645-1653`) is the case that needs it.
    pub fn subscribe_typed_bus(&mut self, topic: impl Into<String>) {
        self.typed_bus_topics.push(topic.into());
    }

    /// The typed-bus topics declared through [`Self::subscribe_typed_bus`], moved out.
    pub(crate) fn take_typed_bus_topics(&mut self) -> Vec<String> {
        std::mem::take(&mut self.typed_bus_topics)
    }

    pub fn subscriptions(&self) -> Subscriptions {
        self.subs
    }

    /// The live providers declared through [`Self::register_provider_live`], moved out.
    pub(crate) fn take_live_providers(
        &mut self,
    ) -> Vec<(String, Arc<dyn cyrup_provider::Provider>)> {
        std::mem::take(&mut self.live_providers)
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
            self.branch_change,
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

    /// Receive a TYPED bus event this extension subscribed to through
    /// [`InitApi::subscribe_typed_bus`], inline inside the emitter's
    /// [`crate::SharedBus::emit_typed`] call — pi's synchronous `emitter.emit(channel, data)`
    /// (`core/event-bus.ts:15-17` @v0.87.1), where a listener answers `data` and the emitter reads
    /// the answer as soon as `emit` returns.
    ///
    /// [CYRUP-DELTA] upstream's listener duck-types the payload (`typeof request.claim ===
    /// "function"`); here the listener downcasts `event` to the concrete request type the topic's
    /// owner publishes, so a payload of the wrong shape never reaches a listener that would answer
    /// it, and the request's API is checked at compile time. Native-only: a WASM guest's store is
    /// not re-enterable inside another extension's dispatch (see [`crate::bus`]), so a typed
    /// request never crosses into a guest.
    ///
    /// Synchronous on purpose, so it must not block: it runs on the emitter's stack, which is the
    /// point. Default: ignore.
    fn on_typed_bus_event(&self, _topic: &str, _event: &dyn std::any::Any) {}

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

    /// Whether this built-in is **replaceable**: left out of the loaded set when another extension
    /// registers a tool, command or flag with a name it registers, instead of the two colliding
    /// (pi `InlineExtension.replaceable`, `core/extensions/types.ts:2018-2024` and
    /// `omitReplacedExtensions`, `core/resource-loader.ts:116-153` @v1.0.1). pi's `codemode`,
    /// `tool-search` and `mcp` built-ins are replaceable, so a third-party extension that registers
    /// `codemode`, `tool_search` or `/mcp` takes over cleanly. [`crate::replaceable`] states the
    /// rule.
    ///
    /// `init` still runs for a replaceable built-in that is left out, as the factory does upstream,
    /// so it should register only tools, commands, flags and event handlers. Default `false`.
    fn replaceable(&self) -> bool {
        false
    }

    /// Whether this built-in is **hidden** from the startup `[Extensions]` listing. It is still
    /// loaded, still in [`crate::ExtensionHost::loaded_ids`], and still dispatched; only the list
    /// the user is shown leaves it out.
    ///
    /// pi marks every extension its `builtin:<name>` branch loads `hidden` — the default and the
    /// `-e builtin:<name>` form alike (`extension.hidden = true` in `loadExtensionPaths`,
    /// `core/resource-loader.ts:741` @f1b2e77f5) — and the interactive startup panel lists only
    /// `!extension.hidden` (`modes/interactive/interactive-mode.ts`). Hidden is a property of HOW
    /// the extension was loaded upstream. cyrup has that load path too (EXT-094: the binary's
    /// `session_launch::BuiltinSelection` attaches `llama.cpp`, `codemode`, `tool-search` and `mcp`
    /// only as `builtin:<name>`), but the host reads hidden from the native, so each of those four
    /// natives declares it. Default `false`: anything else, an embedder-supplied inline factory
    /// included, is listed, as in pi (whose named inline factory may still declare `hidden`,
    /// `resource-loader.ts:1148`). EXT-092: the binary's test
    /// `every_attached_native_is_hidden_exactly_when_pi_loads_its_counterpart_as_a_builtin`
    /// (`crates/cyrup/src/session_launch.rs`) derives each attached native's expected state from
    /// that load path — hidden iff attached as a `builtin:<name>`, default or `-e` — and fails on a
    /// native that disagrees.
    fn is_hidden(&self) -> bool {
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
    /// name, or a handler that in pi would THROW). pi shows a thrown handler as `command:<name>`
    /// plus its message (`agent-session.ts:2087`), and the runner formats `Err` the same way, so a
    /// handler that mirrors a throw returns [`ExtError::CommandFailed`] (which prints its message
    /// bare) and issues no notice of its own. A user-facing error pi's handler REPORTS rather than
    /// throws is better sent as a self-issued `Error` notify plus `Ok(None)`, which keeps the
    /// wording under the handler's control instead of wrapping it in the `command:<name>: `
    /// prefix.
    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        Err(ExtError::Component(format!(
            "native extension has no handler for command `{name}`"
        )))
    }

    /// Dynamic argument completions for a command this extension registered and opted in with
    /// [`InitApi::add_autocomplete`] — the native tier's half of
    /// `RegisteredCommand.getArgumentCompletions?(argumentPrefix)`
    /// (`core/extensions/types.ts:1166` @v0.83.0), whose guest half is the
    /// `get-argument-completions` export (`wit/world.wit:250`).
    ///
    /// `name` is the REGISTERED name, not the invocation name — the same resolution
    /// [`crate::ExtensionHost::command_completions`] does for `execute_command`, so a
    /// disambiguated `deploy:2` still reaches the handler's own `deploy` arm.
    ///
    /// Default: no completions, which is upstream's unset field. Overriding it without also
    /// calling `add_autocomplete` is inert — the opt-in table is what reaches the front-end.
    async fn argument_completions(
        &self,
        _name: &str,
        _prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        Ok(Vec::new())
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
        Err(ExtError::Component(format!(
            "native extension has no handler for shortcut `{key}`"
        )))
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

    /// [`Self::render_call`] with the display inputs upstream passes every renderer (EXT-006;
    /// `ToolDefinition.renderCall(args, theme, context)`, `extensions/types.ts:491` @v0.84.4, and
    /// `MessageRenderer = (message, options, theme)`, `:1213-1217`).
    ///
    /// Defaults to [`Self::render_call`], which is the whole point: a renderer whose output does
    /// not vary with the expand toggle or the theme implements the two-argument form and is done.
    /// Override THIS one to branch on `opts` — the host re-invokes it whenever the options move
    /// (`cyrup_tui::App::refresh_extension_renders`), so the branch is live, not frozen.
    ///
    /// The richer alternative is [`Self::render_live`], which is re-rendered per FRAME with the
    /// terminal width as well; this hook exists for a renderer that wants the options without
    /// owning a component.
    fn render_call_under(
        &self,
        key: &str,
        call: &serde_json::Value,
        _opts: &crate::RenderOptions,
    ) -> Option<serde_json::Value> {
        self.render_call(key, call)
    }

    /// The result-side companion of [`Self::render_call_under`] (Pi `renderResult(result, options,
    /// theme, context)`, `extensions/types.ts:493-498` @v0.84.4 — the ONE upstream renderer whose
    /// options bag also carries `isPartial`).
    fn render_result_under(
        &self,
        key: &str,
        result: &serde_json::Value,
        _opts: &crate::RenderOptions,
    ) -> Option<serde_json::Value> {
        self.render_result(key, result)
    }

    /// The entry-side companion of [`Self::render_call_under`] (Pi `EntryRenderer = (entry,
    /// options: EntryRenderOptions, theme)`, `extensions/types.ts:1219-1223` @v0.84.4).
    fn render_entry_under(
        &self,
        custom_type: &str,
        entry: &serde_json::Value,
        _opts: &crate::RenderOptions,
    ) -> Option<serde_json::Value> {
        self.render_entry(custom_type, entry)
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
    ///
    /// # This handler MUST NOT reach a blocking capability
    ///
    /// UW-7 hazard H1, and the reason it is stated on the trait rather than on one implementor.
    /// The consume-or-deliver answer is needed BEFORE the editor sees the key, so the TUI awaits
    /// [`crate::ExtensionHost::terminal_input`] on the very task that services `ui_rx`
    /// (`cyrup-tui/src/app/run_action.rs`, `App::on_input_event`). Any handler that calls
    /// [`crate::host::HostServices::open_overlay`] — whose contract is *"BLOCK until the user
    /// closes it"* (`crate::host::HostServices::open_overlay`) — or `confirm`/`select`/`input`,
    /// which block on `ui_roundtrip`'s one-shot reply, blocks THAT task; and that task is the only
    /// one that can ever unblock it. The whole TUI wedges, keyboard included.
    ///
    /// The shortcut path escapes this by spawning (`run_action.rs`, the
    /// `AppAction::ExtensionShortcut` arm); the input path cannot, because it needs the answer
    /// synchronously. A handler that wants to open a modal must therefore `tokio::spawn` it and
    /// return `consume` immediately — which is exactly what upstream does, on a detached
    /// microtask: `void Promise.resolve().then(() => this.openInspector(selectedKey))`
    /// (`pi-subagents/src/tui/fleet-status.ts:741-750` @v0.68.0).
    fn on_terminal_input(&self, _data: &str) -> Option<crate::TerminalInputResult> {
        None
    }

    /// The git branch changed (EXT-064; pi `ReadonlyFooterDataProvider.onBranchChange`'s callback,
    /// invoked from `notifyBranchChange`, `core/footer-data-provider.ts:197-199` @v0.87.1). Only
    /// consulted on a native that declared [`InitApi::subscribe_branch_change`], and only on a REAL
    /// change — upstream calls its callbacks inside `if (this.cachedBranch !== next)` (`:224-227`).
    ///
    /// `branch` is upstream's `getGitBranch()` tri-state verbatim: a branch name, the literal
    /// `"detached"`, or `None` outside a repo (`:126-132`).
    ///
    /// Sync, and subject to the same "must not block" hazard as [`Self::on_terminal_input`]: it is
    /// driven from the TUI's own poll arm.
    fn on_branch_change(&self, _branch: Option<&str>) {}

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

    /// Resolve a LIVE component for a custom MESSAGE or ENTRY this extension registered a renderer
    /// for (Pi `registerMessageRenderer` returning a `Component`, `pi-intercom/index.ts:1816-1820`).
    /// `key` is the custom type; `payload` is the serialized message (or entry).
    ///
    /// This is the tier the string-returning [`Self::render_call`] / [`Self::render_entry`] cannot
    /// express: those are invoked ONCE, at fold time, with no width, no theme and no expansion, so
    /// their output is frozen at whatever the terminal happened to be when the message arrived. A
    /// component is resolved once and re-rendered per frame, which is what makes a resize re-wrap
    /// and an expand toggle open a card in place.
    ///
    /// `None` — upstream's `return undefined` for a payload carrying no `details` — falls through to
    /// the string hooks and then to the host's own framing. Consulted BEFORE them, exactly as the
    /// native tool-renderer fast tier is consulted before the registry dispatch.
    ///
    /// Sync, and a PANIC is contained by the host, for the same reasons as [`Self::render_call`].
    fn render_live(
        &self,
        _key: &str,
        _payload: &serde_json::Value,
    ) -> Option<std::sync::Arc<dyn RenderedComponent>> {
        None
    }

    /// The component form of [`Self::render_call`] for a tool this extension declared a renderer
    /// for (pi `ToolDefinition.renderCall` returning a `Component`). Consulted BEFORE the JSON
    /// hooks; `None` falls through to them. Sync, and a PANIC is contained by the host, for the
    /// same reasons as [`Self::render_call`].
    ///
    /// Unlike [`Self::render_live`] the component hands the host a tree
    /// ([`crate::RenderedTree`]), so the host wraps and truncates styled text at the live width.
    fn render_call_tree(
        &self,
        _key: &str,
        _call: &serde_json::Value,
        _opts: &crate::RenderOptions,
    ) -> Option<std::sync::Arc<dyn crate::RenderedTree>> {
        None
    }

    /// The result-side companion of [`Self::render_call_tree`] (pi `renderResult(result, options,
    /// theme, context)`). `result` is `{content, details}`; `opts` carries `expanded`, `isPartial`
    /// and `isError`. Called again for every partial result a streaming tool reports, so a list that
    /// grows while the tool runs is a new component each time.
    fn render_result_tree(
        &self,
        _key: &str,
        _result: &serde_json::Value,
        _opts: &crate::RenderOptions,
    ) -> Option<std::sync::Arc<dyn crate::RenderedTree>> {
        None
    }

    /// Supply a per-call command-execution backend for a `user_bash` command this extension just
    /// serviced — Pi `UserBashEventResult.operations`
    /// (`packages/coding-agent/src/core/extensions/types.ts:1136-1142` @v0.84.4, the field at `:1139`:
    /// *"Custom operations to use for execution"*), consumed by the RPC host at
    /// `packages/coding-agent/src/modes/rpc/rpc-mode.ts:581` (`operations: eventResult?.operations`)
    /// and by the interactive `!`/`!!` handler at
    /// `packages/coding-agent/src/modes/interactive/interactive-mode.ts:6524`.
    ///
    /// Consulted ONLY on the extension whose [`HookOutcome::Handled`] won the `user_bash`
    /// reduction, and only when that value carried no `result` — upstream reads exactly one
    /// `UserBashEventResult`, the first truthy handler's (`extensions/runner.ts:1005-1032`), and its
    /// `result` short-circuits execution before `operations` is ever looked at
    /// (`rpc-mode.ts:571-576`; `interactive-mode.ts:6471-6499`).
    ///
    /// Native-only by construction, for the same reason as [`Self::render_live`]: a
    /// [`cyrup_tools::ops::BashOperations`] is an object with an `exec` METHOD, and ADR-0002
    /// (`docs/adr/ADR-0002-extension-io-is-serde.md`) makes extension I/O values rather than
    /// references, so a WASM guest cannot hand one back across the WIT boundary. A guest supplies
    /// its backend the other way instead — the `register-bash-operations` import + keyed
    /// `bash-operations-exec` export round-trip (DRIFT-004), which
    /// [`crate::ExtensionHost::user_bash_operations`] resolves as its second tier. THIS method is
    /// the native tier of the same question.
    ///
    /// `None` — upstream's absent `operations` — falls through to
    /// `createLocalBashOperations({ shellPath })` (`core/agent-session.ts:2782`'s `??`), i.e. the
    /// local shell. The arguments repeat the live `UserBashEvent` fields
    /// (`extensions/types.ts:813-821`) so a stateless extension can decide per command without
    /// stashing what [`Self::on_event`] saw. Sync, and a PANIC is contained by the host and treated
    /// as `None`, so a broken extension can never break a user's `!` command.
    fn user_bash_operations(
        &self,
        _command: &str,
        _exclude_from_context: bool,
        _cwd: &str,
    ) -> Option<Arc<dyn cyrup_tools::ops::BashOperations>> {
        None
    }

    /// Late-bind the live `Arc<dyn HostServices>` backend (reconciliation §2 item 1 / P-1). Called by
    /// [`crate::ExtensionHost::load_native_with_services`] BEFORE [`Self::init`], handing a native
    /// built-in the SAME capability backend the WASM path already receives (via `discover_and_load`).
    /// The default is a no-op — a built-in that needs none simply ignores it. A built-in that DOES
    /// need late, out-of-`HostCtx` reach (a background tokio task that must resolve the live session
    /// id/file, open a dialog, or inject a turn-triggering message) overrides this to STASH the `Arc`
    /// in a [`crate::host::HostServicesSlot`] (or a `Mutex` of its own) and NOT in a `OnceLock`: a
    /// session replacement (`/new`, RPC `new_session`, a second ACP `session/new`) builds the
    /// replacement's `LiveHostServices` and calls this again on the SAME extension object, and a
    /// set-once slot keeps serving the session that was replaced. The captured `Arc` is a shared handle to
    /// the one `LiveHostServices` the session late-attaches its manager / ui sink / inject sink to, so
    /// capturing it early (before those attachments) is correct: the built-in observes them through
    /// the `Arc`'s interior mutability when the background task actually runs. Gated on `wasm-host`
    /// because the [`crate::host::HostServices`] trait itself only exists with the capability host.
    #[cfg(feature = "wasm-host")]
    fn set_host_services(&self, _services: Arc<dyn crate::host::HostServices>) {}

    /// Late-bind the post-`init` registration handle (HA-1 / MCP-037). Called by
    /// `ExtensionHost::load_native_inner` BEFORE [`Self::init`], for the same reason
    /// [`Self::set_host_services`] binds before it: an `init`-spawned background task must already
    /// hold the handle when it runs.
    ///
    /// The default is a no-op — a built-in whose whole surface is known at `init` simply ignores
    /// it. One that discovers tools later (cyrup-mcp connecting a server mid-session) STASHES the
    /// `Arc` in its own interior-mutable slot, exactly as [`Self::set_host_services`] is stashed.
    ///
    /// Deliberately NOT `cfg(feature = "wasm-host")`, unlike the method above: [`LateRegistrar`] is
    /// typed against nothing behind that feature precisely so both build arms have it. See the
    /// trait's own doc, and EXT-060 for what the gated version of this mistake cost last time.
    fn set_late_registrar(&self, _registrar: Arc<dyn LateRegistrar>) {}
}

/// Which of pi's two extension tiers a [`QuarantinedNative`] stands in for, so `--no-extensions`
/// treats the placeholder exactly as it would have treated the real built-in (SEAM-071/SEAM-074).
///
/// Getting this wrong is observable in one direction only, and it is the bad one: a placeholder
/// that claims the inline tier while the extension it replaces is ambient survives a flag that
/// drops the real thing, so `cyrup --no-extensions` would report a load failure for an extension
/// that was never going to load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuarantinedTier {
    /// The quarantined built-in declares [`NativeExtension::is_ambient`] — it stands in for an
    /// upstream INSTALLED package, the tier `noExtensions` collapses.
    Ambient,
    /// The quarantined built-in stands in for pi's inline-factory tier, which no flag about
    /// discovery touches.
    Inline,
}

/// A native built-in whose registration was REFUSED before the extension object could be built —
/// pi's `load.discard()` + `Failed to load extension: <message>`, available to an embedder that
/// has no factory to throw out of.
///
/// # Why this type exists
///
/// Upstream runs every extension factory inside a `try`. A factory that throws is
/// `load.discard()`ed — every registration it had already made is rolled back — the loader records
/// `Failed to load extension: <message>`, and the loop goes on to the next extension
/// (`initializeExtension`, `pi/packages/coding-agent/src/core/extensions/loader.ts:613-630` and
/// `loadExtension`'s catch at `:655` @v1.0.1; the inline tier has the same catch of its own at
/// `core/resource-loader.ts:1130-1141`). The session still builds, with that one extension absent.
///
/// `cyrup-session-svc`'s build loop already contains a native's `init` failure the same way
/// (EXT-S01): it records the per-extension diagnostic, keeps loading the rest, and marks it fatal
/// so the bin reports it and exits 1 in every mode, which is pi's `main.ts:914-922`.
///
/// What had no equivalent is a refusal that happens EARLIER than `init`. A native built-in is
/// constructed by the embedder and handed to the builder, so a built-in that declines to exist at
/// all — a `config.json` it will not accept, an env payload that does not decode — has no factory
/// for the loader to catch. The embedder's only options were to drop the extension silently (a
/// fail-open) or to carry the error out of the attach point (which aborts the whole launch, where
/// upstream loses one extension). This type is the third option, and it is upstream's: it carries
/// the refused built-in's own id, registers nothing at all — so `discard()` is trivially exact —
/// and returns the refusal from [`NativeExtension::init`], which is the one path the EXT-S01
/// containment loop already treats as pi's per-extension load failure.
pub struct QuarantinedNative {
    id: ExtensionId,
    /// The refusal's own message. Surfaced verbatim, inside the host's
    /// `Failed to load extension "<id>": …` frame — upstream's message is the thrown `Error`'s.
    reason: String,
    tier: QuarantinedTier,
}

impl QuarantinedNative {
    /// Quarantine the built-in `id` with `reason` as its load-failure message.
    ///
    /// `tier` must be the tier the REAL built-in declares — see [`QuarantinedTier`].
    pub fn new(id: ExtensionId, reason: impl Into<String>, tier: QuarantinedTier) -> Self {
        Self {
            id,
            reason: reason.into(),
            tier,
        }
    }
}

#[async_trait::async_trait]
impl NativeExtension for QuarantinedNative {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Mirrors the refused built-in's own tier, so the placeholder lives and dies with it under
    /// `--no-extensions`.
    fn is_ambient(&self) -> bool {
        matches!(self.tier, QuarantinedTier::Ambient)
    }

    /// The throw. [`ExtError::Registration`] is the variant for a registration pi's
    /// `ExtensionAPI` refuses by throwing inside the factory, and it `Display`s its message
    /// verbatim, so the host renders exactly upstream's
    /// `Failed to load extension "<id>": <message>`.
    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Err(ExtError::Registration(self.reason.clone()))
    }

    /// Unreachable in practice: a built-in whose `init` failed is not in the dispatch set. Declared
    /// because the trait requires it, and inert so that it stays harmless if it ever is reached.
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
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

/// The post-`init` registration handle (HA-1 / MCP-037). pi's `api.registerTool` and
/// `api.registerCommand` are legal from ANY live handler — the api object an extension captured at
/// load is the same object for the session's life (`extensions/loader.ts:267-289` @v0.83.0), and
/// `registerTool` ends with an unconditional `runtime.refreshTools()`. A native extension had no
/// equivalent: its only host handles are the `Arc<dyn HostServices>` late-bound by
/// [`NativeExtension::set_host_services`] and the per-dispatch [`HostCtx`], and `HostServices`'
/// five tool-shaped verbs (`active_tools`, `all_tool_names`, `set_active_tools`, `all_tools`,
/// `commands`) are all read-or-restrict — none ADDS, and `set_active_tools` cannot activate a name
/// that was never registered. The WASM tier reaches the same registry through its `registration`
/// WIT import, so this was a two-tier asymmetry in one verb, not an absent capability.
///
/// [`InitApi`] is a COLLECTOR — it accumulates into `Vec`s that `load_native_inner` drains once —
/// which is why it is `&mut` and only exists during `init`. This writes THROUGH to the registry
/// immediately. That is the whole difference between the two.
///
/// **Feature-independent, deliberately**, and this is load-bearing rather than stylistic: the
/// obvious shape — a sibling of [`NativeExtension::set_host_services`] — inherits that method's
/// `cfg(feature = "wasm-host")`, and so does its only caller
/// [`crate::ExtensionHost::load_native_with_services`]. EXT-060 records what that costs: the
/// EXT-005 rich-ctx fix was typed against [`crate::host::HostServices`], so a
/// `--no-default-features` build silently lost the whole fix with no diagnostic. [`HostCtxSource`]
/// above is the repair, and this trait is the same repair applied before the fact — the manifest
/// explicitly invites a host built without Wasmtime, and whether a native can register a tool must
/// not depend on which host features were compiled in.
pub trait LateRegistrar: Send + Sync {
    /// pi `api.registerTool` from a live handler. Lands in the executable tool map and raises the
    /// tools-dirty flag; [`crate::ExtensionHost::refresh_tools`] reports it and the next turn
    /// boundary surfaces it to the model.
    fn register_tool(&self, tool: Arc<dyn Tool>) -> Result<(), ExtError>;

    /// pi `api.registerCommand` from a live handler. Re-registering an existing name UPDATES it in
    /// place rather than appending a second row — see [`crate::registry::ExtensionRegistry::register_command`].
    fn register_command(&self, name: String, desc: CommandDescriptor) -> Result<(), ExtError>;

    /// Declare that this extension renders the tool named `tool_name` (MCP-036). Split from
    /// [`Self::register_tool`] for the same reason [`InitApi`] splits them: a native tool is an
    /// already-executable `Arc<dyn Tool>` with no descriptor to carry `has_renderer`.
    fn register_tool_renderer(&self, tool_name: String) -> Result<(), ExtError>;

    /// pi `api.registerProvider` from a live handler or background task, with the provider object
    /// itself ([`InitApi::register_provider_live`] is the `init`-time form). Lands in the model
    /// registry immediately; registering an id that is already registered REPLACES it and bumps the
    /// registry generation, so the model lists see the new catalog on their next read.
    ///
    /// Required, like every other method here: pi's `registerProvider` always reaches the model
    /// registry (`loader.ts:449-462`), so an implementor either does the registration or returns
    /// an `Err` of its own. There is no default that would let a registrar compile and then refuse.
    fn register_provider_live(
        &self,
        id: String,
        provider: Arc<dyn cyrup_provider::Provider>,
    ) -> Result<(), ExtError>;

    /// pi `api.unregisterProvider(name)` (`extensions/types.ts:1819` @v0.99.2-17), for a provider THIS extension
    /// registered (live or JSON): removes it from the model registry and returns whether it was
    /// present. A provider another extension owns is left alone and reads as `false`. Required for
    /// the same reason as [`Self::register_provider_live`].
    fn unregister_provider(&self, id: &str) -> Result<bool, ExtError>;

    /// pi `api.registerVirtualModel` from a live handler or background task
    /// ([`InitApi::register_virtual_model`] is the `init`-time form). Lands in the session's
    /// virtual-model registry immediately and bumps its generation, so the next catalog read lists
    /// it; re-registering a `(provider, id)` REPLACES it in place.
    ///
    /// Required for the same reason as [`Self::register_provider_live`]: upstream's post-bind
    /// `runtime.registerVirtualModel` is a direct call into the model registry
    /// (`extensions/runner.ts:539`), so an implementor either does the registration or returns an
    /// `Err` of its own. There is no default that would let a registrar compile and then refuse.
    fn register_virtual_model(
        &self,
        definition: cyrup_provider::VirtualModelDefinition,
    ) -> Result<(), ExtError>;

    /// pi `api.unregisterVirtualModel(provider, id)` (`extensions/types.ts:1875-1876` @v1.0.4), for
    /// a virtual model THIS extension registered: removes it and returns whether it was present. A
    /// pair another extension owns is left alone and reads as `false`.
    ///
    /// Scoped to the ONE pair, never to the provider: upstream's docs page states that
    /// `pi.unregisterProvider()` does not remove virtual models, and the converse holds too.
    fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<bool, ExtError>;

    /// The extension this handle registers on behalf of. The host binds it at construction, so an
    /// extension holding the handle cannot register under another extension's id — the reason this
    /// is a narrow capability rather than a `Weak<ExtensionHost>`.
    fn owner(&self) -> ExtensionId;
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
    pub fn new(inner: Arc<dyn NativeExtension>, subs: Subscriptions, ctx: HostCtx) -> Self {
        let id = inner.id();
        Self {
            id,
            subs,
            ctx,
            inner,
            ctx_source: None,
        }
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

    /// The sanctioned-wait gate for this native handler: its ctx's shared [`SanctionedWaitGate`].
    /// The dispatcher's budget watchdog reads it to forgive a DECLARED long wait (a human, P-3; a
    /// watchdog model review, UW-3 — see [`HostCtx::begin_sanctioned_wait`]). A native that declares
    /// nothing leaves it idle and keeps the fail-fast budget.
    fn human_wait_gate(&self) -> Option<Arc<HumanWaitGate>> {
        Some(self.ctx.human_wait_gate())
    }

    async fn invoke_event(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        self.invoke_in(self.dispatch_ctx(), ev, cancel).await
    }

    async fn invoke_nested_event(
        &self,
        parent: &cyrup_core::ToolCallId,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        let ctx = self.dispatch_ctx().with_parent_tool_call_id(parent.clone());
        self.invoke_in(ctx, ev, cancel).await
    }
}

impl NativeHandle {
    /// Run the handler with `ctx`, containing a panic and racing `cancel`.
    async fn invoke_in(
        &self,
        ctx: HostCtx,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        // Containment: catch a panicking handler (R-08-036). `AssertUnwindSafe` is sound here — on
        // a caught unwind we discard the handler's state and surface an error; we never resume it.
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

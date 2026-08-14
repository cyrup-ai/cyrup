//! `LiveHostServices` — the concrete [`cyrup_ext::host::HostServices`] backend the session injects
//! (arch-08 §5.6; retires the cyrup-ext "outer-layer" ledger row). cyrup-ext ships the trait plus a
//! deny-all [`cyrup_ext::host::DenyServices`] default and a [`cyrup_ext::host::RecordingServices`]
//! test double; this is the REAL backend wired to the running session's provider + active model +
//! a command-tier control sink, so a loaded extension's `models`/`session`/`control` capabilities
//! reflect live runtime state instead of returning empty/denied.
//!
//! The `HostServices` trait methods are synchronous (the guest is suspended across the host call),
//! while the session's manager is async-locked. `LiveHostServices` therefore reads from a small
//! sync snapshot the session pushes on model/state changes, plus the provider's (sync) model list.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, EntryId, ModelRef, Tool};
use cyrup_ext::caps::http::HttpCaps;
use cyrup_ext::caps::proc::ProcCaps;
use cyrup_ext::host::{
    ControlOp, DialogOptions, ExecOutput, HostServices, HttpRequest, HttpResponse,
    HttpStreamResponse, HumanInteractionLock, InteractiveOverlay, NotifyKind, ProcSpawnSpec,
};
use cyrup_provider::Provider;
use cyrup_session::manager::SessionManager;
use cyrup_tools::{ArgvSpec, ExitStatus, ProcOps};
use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

use crate::event::AgentSessionEvent;
use crate::tools::DynamicToolState;

/// Fallback ceiling for the `exec` grant's full round trip (spawn through exit) when the guest
/// supplied NO `opts.timeoutMs` (or gave `0`, which — like `http-client`'s `timeout_ms` — means "no
/// explicit timeout", not an instant one; see [`LiveHostServices::exec`]'s `.filter(|ms| *ms > 0)`
/// guard). Pi's own `execCommand` (exec.ts:74-79) is ALSO genuinely unbounded absent a `timeout`, so
/// this is not a Pi-parity gap — but Pi's caller can still interrupt an untimed call live via
/// `options.signal.addEventListener("abort", killProcess, {once: true})` (exec.ts:65-72), a listener
/// that stays LIVE in Node's event loop for the whole call. Cyrup's equivalent (`opts.signalId`) can
/// only pre-cancel an ALREADY-aborted signal at call entry (`host/live.rs`'s `exec` import): the guest
/// is wasm-suspended for the entire synchronous `block_in_place`+`block_on` bridge below, so nothing
/// can observe or act on a signal that aborts mid-run. That asymmetry means an untimed `exec` here has
/// NO live escape hatch at all — unlike Pi, and unlike this file's own sibling `http-client` grant
/// (`cyrup_ext::caps::http::DEFAULT_REQUEST_TIMEOUT`, added for the identical "unbounded blocking-pool
/// thread" concern). Mirrors that constant's rationale and magnitude exactly: deliberately generous
/// (comfortably above any realistic legitimate command runtime) while still guaranteeing the call can
/// never hang literally forever. An EXPLICIT non-zero `opts.timeoutMs` from the guest always wins;
/// this only fills the gap when none was given. Fed straight into `ProcOps::exec_argv`'s existing
/// `timeout: Option<Duration>` (rather than wrapped in an outer `tokio::time::timeout`) so a fallback
/// firing still goes through the SAME SIGTERM-then-grace-then-SIGKILL escalation
/// (`cyrup-tools/src/ops/local.rs`) as an explicit guest timeout — not an ungraceful `kill_on_drop`
/// SIGKILL from abandoning the future outright.
const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(120);

/// A command-tier control sink: a loaded extension's `control` import (new/switch/fork/…) is routed
/// here so the runtime can act on it (Pi `createCommandContext`, agent-session.ts:1158). Set by the
/// runtime once it owns the session; until then control ops are reported as unavailable.
pub type ControlSink = Arc<dyn Fn(ControlOp) -> Result<(), String> + Send + Sync>;

/// A rebuilt active-tool push: the new tool array + the rebuilt system prompt a guest `setActiveTools`
/// produced (Pi `setActiveToolsByName` output, agent-session.ts:850-854), queued for the async agent
/// push in [`crate::AgentSession::apply_pending_control`].
type ActiveToolsPush = (Vec<Arc<dyn Tool>>, String);

/// Which dialog family a [`UiRequest`] carries (Pi `ExtensionUIContext.{confirm,input,select,editor}`,
/// types.ts:127-133,216).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKind {
    Confirm,
    Input,
    Select,
    Editor,
}

/// The value a dialog renderer sends back to the wasm-suspended guest (the REPLY half of the
/// request/reply [`UiSink`]). `Confirm` -> `confirm` bool; `Text` -> `input`/`editor`/`select`
/// `option<string>` (Pi `select(title, options, opts): Promise<string|undefined>`, types.ts:127,
/// and the WIT `select` return, world.wit:259 — the chosen option STRING, zero index bookkeeping).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiReply {
    Confirm(bool),
    Text(Option<String>),
}

/// A single dialog request routed from a loaded extension's `ui.{confirm,input,select,editor}`
/// capability to the mode's dialog renderer (the interactive TUI selector, or the RPC
/// `extension_ui_request`/`extension_ui_response` round-trip). This is the REQUEST/REPLY inverse of
/// the fire-and-forget [`ControlSink`]: the guest coroutine is wasm-suspended across the SYNC host
/// call (Pi's `ExtensionUIContext` methods RETURN a value the extension awaits, types.ts:127-133,216),
/// so the host BLOCKS on `reply` until the renderer answers, rather than queueing and returning `()`.
pub struct UiRequest {
    pub kind: UiKind,
    /// The dialog title (Pi `title`) — for ALL FOUR kinds, including `editor` (Pi `editor(title,
    /// prefill)`, types.ts:216; world.wit:267).
    pub prompt: String,
    /// For `select`, the JSON array of option strings (Pi `options`); `Null` for the other kinds.
    pub options: Value,
    /// `confirm`'s message body (Pi `confirm(title, message, opts)`, rpc-types.ts:232); `editor`'s
    /// seed text (Pi `prefill`, rpc-types.ts:241); empty string for `input`/`select`.
    pub message: String,
    /// `input`'s placeholder (Pi `input(title, placeholder, opts)`, rpc-types.ts:233-240); `None` for
    /// the other kinds, or when the guest omitted it (L4 review §2.7).
    pub placeholder: Option<String>,
    /// The Pi `ExtensionUIDialogOptions` bag (`{timeoutMs, signalId}`, types.ts:89).
    pub opts: DialogOptions,
    /// The one-shot the renderer fulfils to resume the suspended guest.
    pub reply: tokio::sync::oneshot::Sender<UiReply>,
}

/// A request/reply dialog sink: a loaded extension's `ui.*` capability is routed here so the active
/// mode's renderer (TUI / RPC) can service it and reply. Set by the mode entry point via
/// [`LiveHostServices::set_ui_sink`]; absent (`None`) in headless (print/json), where the ui methods
/// fall back to the deny defaults (== Pi `noOpUIContext`, runner.ts:230-261).
pub type UiSink = UnboundedSender<UiRequest>;

/// A live interactive modal an extension handed to the host, plus the one-shot the renderer fulfils
/// once the user closes it.
///
/// The SECOND request/reply shape on this seam (the first is [`UiRequest`]), and the difference is
/// duration, not direction: a `UiRequest` is answered by one keystroke sequence and yields a value,
/// while an `OverlayRequest` transfers OWNERSHIP of a component the renderer then drives — paint,
/// keystroke, paint, … — for as long as the user keeps it open. The reply carries no value because
/// pi's own `ctx.ui.custom<undefined>(…)` carries none either (`pi-subagents/src/tui/fleet.ts:869`):
/// the overlay talks to the user directly, so there is nothing left to return when it closes.
pub struct OverlayRequest {
    /// The extension-owned component (see [`cyrup_ext::InteractiveOverlay`]).
    pub overlay: Box<dyn InteractiveOverlay>,
    /// Fulfilled when the modal is torn down, releasing the blocked extension task.
    pub done: tokio::sync::oneshot::Sender<()>,
}

/// The interactive-overlay renderer channel — see [`OverlayRequest`]. Installed by the interactive
/// TUI only ([`LiveHostServices::set_overlay_sink`]).
pub type OverlaySink = UnboundedSender<OverlayRequest>;

/// A fire-and-forget `ui.*` effect a loaded extension pushed via `notify`/`set-status`/`set-widget`/
/// `set-header`/`set-footer`/`set-title`/`set-editor-text`/`paste-editor-text`/`set-tools-expanded`
/// (Pi `ExtensionUIContext` mutators, types.ts:130-275) — the ONE-WAY counterpart to [`UiRequest`]:
/// the guest never blocks on a reply (Pi's own signatures return `void`), so there is no `reply`
/// channel here at all, unlike `UiRequest`.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEffect {
    /// Pi `notify(message, type)`, types.ts:136; RPC wire `method:"notify"` (rpc-mode.ts:149-157).
    Notify { message: String, kind: NotifyKind },
    /// Pi `setStatus(key, text?)`, types.ts:141-142; RPC wire `method:"setStatus"`
    /// (rpc-mode.ts:163-172). `text: None` clears the key.
    SetStatus { key: String, text: Option<String> },
    /// Pi `setWidget(key, content, options?)`, types.ts:164-173; RPC wire `method:"setWidget"`
    /// (rpc-mode.ts:190-206). Cyrup's WIT `set-widget(widget-json)` collapsed Pi's 3-argument
    /// `{key, content, options}` shape into ONE opaque JSON payload (`cyrup-ext-sdk::Ctx::set_widget`
    /// takes `impl Serialize` verbatim) — `widget` carries that payload as-is, not re-split into
    /// Pi's `widgetKey`/`widgetLines`/`widgetPlacement` fields (there is no cyrup-side convention to
    /// re-derive them from).
    SetWidget { widget: Value },
    /// Pi `setHeader(factory)`, types.ts:184. Pi's RPC mode never delivers this over the wire at all
    /// ("Custom header not supported in RPC mode - requires TUI access", rpc-mode.ts:209-211) because
    /// Pi's version takes a TUI component FACTORY; cyrup's WIT `set-header(content: string)` is
    /// plain data (world.wit:272), so it is still delivered on this in-process channel for a future
    /// TUI-mode consumer even though the RPC mode does not forward it onward (see `rpc.rs`).
    SetHeader { content: String },
    /// Pi `setFooter(factory)`, types.ts:174-177; same RPC non-forwarding rationale as `SetHeader`
    /// (rpc-mode.ts:213-215).
    SetFooter { content: String },
    /// Pi `setTitle(title)`, types.ts:187; RPC wire `method:"setTitle"` (rpc-mode.ts:216-223).
    SetTitle { title: String },
    /// Pi `setEditorText(text)`/`pasteEditorText(text)`, types.ts:200-230; RPC wire
    /// `method:"set_editor_text"` (rpc-mode.ts:234-241; `pasteToEditor` falls back to the same
    /// handler, rpc-mode.ts:230-232) — note the wire method name is snake_case, unlike its siblings.
    SetEditorText { text: String, is_paste: bool },
    /// Pi `setToolsExpanded(expanded)`, types.ts:275. Pi's RPC mode never forwards this either
    /// ("Tool expansion not supported in RPC mode - no TUI", rpc-mode.ts:296-298); delivered here for
    /// the same future-TUI-consumer reason `SetHeader`/`SetFooter` are.
    SetToolsExpanded { expanded: bool },
}

/// A fire-and-forget effect sink: the mode's renderer (currently RPC; see `cyrup_modes::rpc::run_rpc`)
/// drains [`UiEffect`]s as they arrive and relays the ones Pi's own RPC mode relays (notify/setStatus/
/// setWidget/setTitle/setEditorText — rpc-mode.ts:149-241) onward to the client. Set by the mode entry
/// point via [`LiveHostServices::set_ui_effect_sink`]; absent (`None`) in headless (print/json), where
/// the effect methods below silently drop (== Pi `noOpUIContext`'s `notify`/`setStatus`/… no-ops,
/// runner.ts:234-244).
pub type UiEffectSink = UnboundedSender<UiEffect>;

/// The sync snapshot the session keeps current for the (sync) host-services reads.
#[derive(Clone, Debug, Default)]
struct LiveSnapshot {
    model: Option<ModelRef>,
    context_window: u64,
    used_tokens: u64,
    session_name: Option<String>,
    thinking_level: Option<String>,
    /// The live session id (P-2), cached at [`LiveHostServices::attach_session`]. Immutable per
    /// session, so the cache is never stale — the sync `session_id()` read returns it directly (no
    /// manager lock), which keeps the id available even to a background caller while an in-progress
    /// turn holds the manager's async lock (the permission spool routes hard on this id).
    session_id: Option<String>,
    /// The last-known persisted session-file path (P-2), cached at attach. Only a FALLBACK for
    /// `session_file()` when the live manager lock is momentarily contended — the file is deferred
    /// until the first assistant message and changes on fork, so the live read is authoritative.
    session_file: Option<PathBuf>,
    /// The agent's CURRENT base system prompt (EXT-005; Pi `getSystemPrompt: () => this.systemPrompt`,
    /// agent-session.ts:2434). Seeded by the builder and re-seeded by every
    /// `AgentSession::push_active_tools` — a `setActiveTools` rebuild changes the prompt, and a guest
    /// reading it back must see the rebuilt one, not the build-time one. The agent's own copy lives
    /// behind an `async` accessor, which a SYNC host-services read cannot await, so it is mirrored.
    system_prompt: Option<String>,
    /// Whether the project is trusted (EXT-005; Pi `isProjectTrusted: () =>
    /// this.settingsManager.isProjectTrusted()`, agent-session.ts:2410). Seeded by the builder from
    /// the resolved `SettingsManager`, so a guest asking mid-session gets the session's REAL verdict
    /// rather than the trait default's conservative `false`.
    project_trusted: bool,
}

/// The live session's activity readback + interrupt, backing the `ctx-state`/`control` imports that
/// only the running session can answer (EXT-005). Pi binds these straight to the session object:
/// `isIdle: () => this.isIdle`, `hasPendingMessages: () => this.pendingMessageCount > 0` and
/// `abort: () => { void this.abort() }` (agent-session.ts:2409-2419).
///
/// A separate trait rather than more snapshot fields because these are LIVE — a mirrored `is_idle`
/// would be stale exactly when it matters (mid-run, which is when a handler asks). Attached by
/// `AgentSession::into_shared` over a weak self-handle, so it never keeps the session alive.
pub trait SessionActivity: Send + Sync {
    /// Whether no agent run (including the post-run retry/compaction/continuation loop) is in
    /// flight (Pi `isIdle`).
    fn is_idle(&self) -> bool;
    /// Queued steering + follow-up message count (Pi `pendingMessageCount`).
    fn pending_message_count(&self) -> usize;
    /// Interrupt the in-flight run NOW. Pi runs `void this.abort()` SYNCHRONOUSLY from the handler
    /// that called `ctx.abort()` (agent-session.ts:2412-2418); deferring it to a turn-boundary drain
    /// would abort a run that has already finished, i.e. nothing at all.
    fn abort(&self);
}

/// A host-originated message injection routed from a background task's `inject_message` to the live
/// session's turn loop (Pi `pi.sendMessage(message, {triggerTurn})` → `sendCustomMessage`,
/// agent-session.ts:1337-1370). The REQUEST payload of the late-bound [`InjectSink`]; carries the
/// fields the trait's [`HostServices::inject_message`] takes so the sink can drive the async
/// append/turn on the live session. Closes R-SA-101 (cyrup-ext-subagents background completion).
///
/// [`HostServices::inject_message`]: cyrup_ext::host::HostServices::inject_message
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectMessage {
    /// The message body (Pi `content`).
    pub content: String,
    /// A custom (non-LLM) message tag when `Some` (Pi `customType`, e.g. `"subagent-notify"`); a
    /// plain user message when `None`.
    pub custom_type: Option<String>,
    /// Whether the message is surfaced to the user (Pi `display`).
    pub display: bool,
    /// Whether to re-enter the agent turn loop over the injected message (Pi `{ triggerTurn: true }`).
    pub trigger_turn: bool,
}

/// A fire-and-forget message-injection sink: [`LiveHostServices::inject_message`] forwards an
/// [`InjectMessage`] here; the installed sink (bound by `AgentSession::into_shared`) spawns the async
/// inject/turn on the live session and returns immediately, so the sync caller never blocks for the
/// whole turn (the same sync→async bridge the [`ControlSink`] uses). `None` until bound (the default
/// host, a headless-by-value session): `inject_message` then reports the seam unavailable, matching
/// the trait's deny default.
pub type InjectSink = Arc<dyn Fn(InjectMessage) -> Result<(), String> + Send + Sync>;

/// The live host-services backend (arch-08 §5.6).
pub struct LiveHostServices {
    provider: Arc<dyn Provider>,
    /// The process backend the `exec` capability grant runs argv (shell:false) commands through
    /// (Pi `execCommand`, exec.ts:34-46). Shared with the session's `bash` seam (the same
    /// [`cyrup_tools::ProcOps`]), so a granted extension execs through the real local process ops.
    proc: Arc<dyn ProcOps>,
    /// The session cwd — the default working directory for an `exec` with no `cwd` option (Pi's
    /// `execCommand(..., opts?.cwd ?? cwd)` where `cwd` is the extension's cwd, loader.ts:317-320).
    cwd: PathBuf,
    /// The `http-client` capability grant's real `reqwest`-backed engine (arch-08 §3.2 draft;
    /// pi-mcp-adapter-port.md §3.2). Gated by the SAME load-time trust check as `exec` (reaching this
    /// backend at all means the guest already passed the trust gate) — no per-call check here either.
    http: HttpCaps,
    /// The `proc` capability grant's real long-lived-child engine (arch-08 §5.2 request/poll bridge;
    /// pi-mcp-adapter-port.md §3.1). Gated by the SAME load-time trust check as `exec`/`http-client`.
    proc_caps: ProcCaps,
    snapshot: Mutex<LiveSnapshot>,
    control: Mutex<Option<ControlSink>>,
    /// The active mode's dialog renderer (interactive TUI / RPC), attached post-build via
    /// [`Self::set_ui_sink`]. A guest's `ui.{confirm,input,select,editor}` capability reaches the SYNC
    /// [`HostServices`] method (the guest is wasm-suspended and cannot await), which forwards a
    /// [`UiRequest`] here and BLOCKS on the one-shot reply. `None` in headless (print/json): the
    /// overrides then fall through to the trait deny defaults (== Pi `noOpUIContext`) and never block.
    ui_sink: Mutex<Option<UiSink>>,
    /// The active mode's fire-and-forget effect drain (interactive TUI / RPC), attached post-build
    /// via [`Self::set_ui_effect_sink`]. A guest's `ui.{notify,set-status,set-widget,set-header,
    /// set-footer,set-title,set-editor-text,paste-editor-text,set-tools-expanded}` capability reaches
    /// the SYNC [`HostServices`] method, which forwards a [`UiEffect`] here and returns immediately —
    /// no reply is awaited, unlike [`Self::ui_sink`]. `None` in headless (print/json): the overrides
    /// then silently drop (== Pi `noOpUIContext`, runner.ts:230-261).
    ui_effect_sink: Mutex<Option<UiEffectSink>>,
    /// The active mode's INTERACTIVE-OVERLAY renderer, attached post-build via
    /// [`Self::set_overlay_sink`]. An extension's `open_overlay` reaches the SYNC [`HostServices`]
    /// method, which forwards an [`OverlayRequest`] here and BLOCKS on the one-shot reply until the
    /// user closes the modal — the request/reply shape of [`Self::ui_sink`], with a whole
    /// interactive session in the middle instead of a single answer. `None` in headless
    /// (print/json) and in RPC, whose wire protocol has no way to stream keystrokes back into a
    /// host-side component; `open_overlay` then returns `false` WITHOUT blocking and the caller
    /// takes its own non-interactive fallback (pi's `!ctx.hasUI` branch).
    overlay_sink: Mutex<Option<OverlaySink>>,
    /// Receiver half of the command-tier control channel (see [`Self::wire_control_channel`]). A
    /// guest's `control` capability call reaches the SYNC [`HostServices::control`] method (the
    /// guest is wasm-suspended and cannot await), which forwards the [`ControlOp`] here; the session
    /// drains + applies it at a command-tier-safe point (Pi runs `createCommandContext` ops directly,
    /// agent-session.ts:1158 — cyrup bridges the sync→async gap via this queue).
    control_rx: Mutex<Option<UnboundedReceiver<ControlOp>>>,
    /// The running session's tree manager, attached post-build via [`Self::attach_session`]. A guest's
    /// `append_entry`/`set_session_name`/`set_label` capability mutates it DIRECTLY (Pi appends
    /// synchronously — `SessionManager.appendCustomEntry`/`setSessionName`/`setLabel`,
    /// agent-session.ts:2265-2279). `None` until attached (default host: no session-mutation authority).
    manager: Mutex<Option<Arc<AsyncMutex<SessionManager>>>>,
    /// Facade events a guest state-mutation queued (`entry_appended`/`session_info_changed`), drained
    /// and fanned out by [`crate::AgentSession::apply_pending_control`] after the guest call settles —
    /// the same sync→async bridge point the control queue uses.
    pending_events: Mutex<Vec<AgentSessionEvent>>,
    /// The session's ONE authoritative dynamic-tool view (Pi `agent.state.tools`), shared with the
    /// facade so a guest's `setActiveTools`/`getActiveTools` capability read+mutates the SAME state
    /// the host/CLI tool-toggle does (Pi binds both to `setActiveToolsByName`/`getActiveToolNames`,
    /// agent-session.ts:2281,2283). Attached post-build via [`Self::attach_dynamic_tools`]; `None`
    /// until then (default host: `active_tools` returns `None` ⇒ the binding uses its own bookkeeping).
    dynamic_tools: Mutex<Option<Arc<Mutex<DynamicToolState>>>>,
    /// The rebuilt `(tools, system_prompt)` a guest `setActiveTools` produced, queued for the ASYNC
    /// agent push [`crate::AgentSession::apply_pending_control`] applies before the next turn (the
    /// guest is wasm-suspended across the SYNC `set_active_tools` call — the same sync→async bridge
    /// `pending_events`/the control queue use). Last write wins (Pi: the last `setActiveTools` wins).
    pending_active_tools: Mutex<Option<ActiveToolsPush>>,
    /// [`DEFAULT_EXEC_TIMEOUT`] in production; overridable ONLY for tests
    /// ([`Self::with_exec_timeout`]) so the fallback-timeout path is exercisable without a real test
    /// waiting the full production duration.
    exec_timeout: Duration,
    /// The late-bound message-injection sink (R-SA-101 / P-2). A guest or a native extension's
    /// background task calls the SYNC [`HostServices::inject_message`]; that forwards an
    /// [`InjectMessage`] here, and the installed sink spawns the async append/turn on the live session
    /// (bound by `AgentSession::into_shared`). `None` until bound (default host / headless-by-value
    /// session): the ui-style sync→async bridge is inert and `inject_message` reports it unavailable.
    inject_sink: Mutex<Option<InjectSink>>,
    /// The ONE session-scoped human-interaction lock (C3, reconciliation §1 / §4 step 6). Created
    /// eagerly at construction (immutable for the session's life) and handed to BOTH companion
    /// extensions through [`HostServices::human_interaction_lock`], so the permission gate's `ask`
    /// dialog and the intercom clarify's supervisor prompt serialize on the SAME lock and can never
    /// prompt the same human at once. Every native handed this backend Arc (via `set_host_services`)
    /// reads the identical lock.
    human_interaction: Arc<HumanInteractionLock>,
    /// The live session's activity readback + interrupt (EXT-005), attached post-build via
    /// [`Self::attach_session_activity`]. `None` on the default/by-value host, where the trait
    /// defaults (idle, no pending messages, a no-op abort) are the honest answers.
    activity: Mutex<Option<Arc<dyn SessionActivity>>>,
    /// EXT-005: `ctx.shutdown()` latched SYNCHRONOUSLY at the capability seam, exactly as Pi does
    /// (`shutdownHandler` is literally `() => { shutdownRequested = true }`, rpc-mode.ts:344-346,
    /// and interactive-mode.ts:1753-1757 sets the field before anything else).
    ///
    /// cyrup routes control ops through a queue that only drains at a turn boundary, which is right
    /// for the ops that need an `async` session effect but WRONG for this one: a shutdown requested
    /// from a background task while the session is idle (or in the window after a run's own drain
    /// has already run) would sit in the queue with no boundary left to drain it, and the host would
    /// never exit. The queued copy is still sent — the drain sets the session's own flag too, and
    /// the latch is a monotone `bool`, so applying it twice is a no-op. Same precedent as
    /// `ControlOp::Abort`, which likewise fires live at this seam AND queues.
    shutdown_requested: std::sync::atomic::AtomicBool,
}

impl LiveHostServices {
    /// Wire a backend to the session's `provider`, process ops (`proc`), and session `cwd`. Model/state
    /// are seeded via [`Self::update_model`] and [`Self::update_state`]; the control sink is attached
    /// later by the runtime. `proc` + `cwd` back the `exec` capability grant (Pi `execCommand`).
    pub fn new(provider: Arc<dyn Provider>, proc: Arc<dyn ProcOps>, cwd: PathBuf) -> Self {
        Self {
            provider,
            proc,
            cwd,
            http: HttpCaps::new(),
            proc_caps: ProcCaps::new(),
            snapshot: Mutex::new(LiveSnapshot::default()),
            shutdown_requested: std::sync::atomic::AtomicBool::new(false),
            control: Mutex::new(None),
            ui_sink: Mutex::new(None),
            ui_effect_sink: Mutex::new(None),
            overlay_sink: Mutex::new(None),
            control_rx: Mutex::new(None),
            manager: Mutex::new(None),
            pending_events: Mutex::new(Vec::new()),
            dynamic_tools: Mutex::new(None),
            pending_active_tools: Mutex::new(None),
            exec_timeout: DEFAULT_EXEC_TIMEOUT,
            inject_sink: Mutex::new(None),
            human_interaction: Arc::new(HumanInteractionLock::new()),
            activity: Mutex::new(None),
        }
    }

    /// Build with a caller-supplied fallback exec timeout (tests only; production always gets the
    /// real [`DEFAULT_EXEC_TIMEOUT`] via [`Self::new`]) — L4 review.
    #[cfg(test)]
    fn with_exec_timeout(provider: Arc<dyn Provider>, proc: Arc<dyn ProcOps>, cwd: PathBuf, exec_timeout: Duration) -> Self {
        Self { exec_timeout, ..Self::new(provider, proc, cwd) }
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Push the active model + its context window (the session calls this on build + `set_model`).
    pub fn update_model(&self, model: ModelRef, context_window: u64, thinking_level: Option<String>) {
        let mut g = Self::lock(&self.snapshot);
        g.model = Some(model);
        g.context_window = context_window;
        g.thinking_level = thinking_level;
    }

    /// Push session-level state (name + last-turn token occupancy) for the read views.
    pub fn update_state(&self, session_name: Option<String>, used_tokens: u64) {
        let mut g = Self::lock(&self.snapshot);
        g.session_name = session_name;
        g.used_tokens = used_tokens;
    }

    /// Seed/refresh the mirrored system prompt + project-trust verdict a guest's `ctx-state` reads
    /// (EXT-005). Called by the builder once and by `AgentSession::push_active_tools` on every
    /// prompt rebuild.
    pub fn update_prompt_state(&self, system_prompt: Option<String>, project_trusted: bool) {
        let mut g = Self::lock(&self.snapshot);
        if system_prompt.is_some() {
            g.system_prompt = system_prompt;
        }
        g.project_trusted = project_trusted;
    }

    /// Attach the live session's activity readback + interrupt (EXT-005). Installed by
    /// `AgentSession::into_shared` over a weak self-handle.
    pub fn attach_session_activity(&self, activity: Arc<dyn SessionActivity>) {
        *Self::lock(&self.activity) = Some(activity);
    }

    /// Attach the command-tier control sink (the runtime owns it once the session is live).
    pub fn set_control_sink(&self, sink: ControlSink) {
        *Self::lock(&self.control) = Some(sink);
    }

    /// Attach the mode's dialog renderer (the interactive TUI selector arm, or the RPC
    /// `extension_ui_request` emitter). Only interactive/rpc call this; headless (print/json) leaves it
    /// `None`, which is what keeps the ui overrides returning the deny defaults WITHOUT blocking — the
    /// absence of a sink IS the headless policy, mirroring Pi's absence of a `uiContext`.
    pub fn set_ui_sink(&self, sink: UiSink) {
        *Self::lock(&self.ui_sink) = Some(sink);
    }

    /// Attach the mode's fire-and-forget effect drain (see [`UiEffectSink`]/[`Self::ui_effect_sink`]).
    /// Only interactive/rpc call this; headless (print/json) leaves it `None`.
    pub fn set_ui_effect_sink(&self, sink: UiEffectSink) {
        *Self::lock(&self.ui_effect_sink) = Some(sink);
    }

    /// Attach the mode's interactive-overlay renderer (see [`OverlaySink`]/[`Self::overlay_sink`]).
    /// ONLY the interactive TUI calls this: it is the one mode that owns a terminal it can route
    /// live keystrokes from. Everything else leaves it `None`, which is what makes
    /// [`HostServices::open_overlay`] answer `false` immediately instead of blocking forever.
    pub fn set_overlay_sink(&self, sink: OverlaySink) {
        *Self::lock(&self.overlay_sink) = Some(sink);
    }

    /// Send one fire-and-forget effect to the attached drain, if any (no-op — matching Pi's
    /// `noOpUIContext` — when unattached). Never blocks: an `UnboundedSender::send` never awaits.
    fn emit_ui_effect(&self, effect: UiEffect) {
        if let Some(sink) = Self::lock(&self.ui_effect_sink).clone() {
            let _ = sink.send(effect);
        }
    }

    /// Route one dialog request to the attached renderer and BLOCK (the guest is wasm-suspended) on the
    /// reply — the request/reply counterpart to the fire-and-forget [`Self::control`]. Returns `None`
    /// when there is no sink (headless: the ui method then yields its deny default WITHOUT blocking),
    /// when the renderer dropped the reply (cancelled / shut down), OR when `opts.timeout_ms` elapses
    /// with no reply — the caller then falls through to its per-kind deny default, matching Pi's
    /// `createDialogPromise`'s host-armed `setTimeout(() => resolve(defaultValue), opts.timeout)`
    /// (`rpc-mode.ts:114-119`), which ALWAYS settles the dialog within `opts.timeout` ms regardless of
    /// client behavior (closes L4 review §2.2). A renderer can ALSO force-resolve an already-open
    /// dialog early by sending on the SAME `reply` one-shot this call is waiting on (e.g. the RPC loop's
    /// `pending` map on `abort`/`abort_retry`, `rpc.rs` — closes L4 review §2.5); that arrives on the
    /// `reply_rx` branch below like any ordinary answer, no extra wiring needed here. Uses the SAME
    /// `block_in_place` + `block_on` pattern the `exec` grant uses ([`Self::exec`]); requires a
    /// multi-threaded runtime, which interactive/rpc guarantee
    /// (`#[tokio::main(flavor = "multi_thread")]`, main.rs:40).
    fn ui_roundtrip(
        &self,
        kind: UiKind,
        prompt: &str,
        options: Value,
        message: String,
        placeholder: Option<String>,
        opts: &DialogOptions,
    ) -> Option<UiReply> {
        let sink = Self::lock(&self.ui_sink).clone()?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = UiRequest {
            kind,
            prompt: prompt.to_string(),
            options,
            message,
            placeholder,
            opts: opts.clone(),
            reply: reply_tx,
        };
        if sink.send(request).is_err() {
            // The renderer (TUI loop / RPC loop) is gone — degrade to the deny default, never a panic.
            return None;
        }
        // `0` means NO timeout, not an instant one — Pi's `createDialogPromise` only arms the timer
        // `if (opts?.timeout)` (`rpc-mode.ts:114`; falsy-zero in JS ⇒ no timer at all), and both real
        // dialog callers double down on the same check (`opts.timeout && opts.timeout > 0`,
        // `extension-selector.ts:51`, `extension-input.ts:54`). Mirror the `> 0` guard the sibling
        // `exec` grant already applies to `timeoutMs` just below ([`Self::exec`]) and the TUI's own
        // countdown applies to the same field (`cyrup-tui/src/app.rs`'s `.filter(|&ms| ms > 0)`).
        let timeout = opts.timeout_ms.filter(|ms| *ms > 0).map(Duration::from_millis);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                match timeout {
                    // Race the reply against a live countdown — Pi's `setTimeout` safety net. Whichever
                    // settles first wins; on timeout the reply half is dropped (never polled again), so
                    // a late answer simply finds its `reply.send` fail harmlessly (`Err`, never a panic).
                    Some(d) => tokio::select! {
                        biased;
                        reply = reply_rx => reply.ok(),
                        () = tokio::time::sleep(d) => None,
                    },
                    None => reply_rx.await.ok(),
                }
            })
        })
    }

    /// Wire the command-tier control channel: a loaded extension's `control` capability (new/switch/
    /// fork/compact/set-model/…) is forwarded onto an in-process queue the session drains via
    /// [`Self::take_pending_control`]. This is the bridge that lets a wasm guest (suspended across
    /// the SYNC `control()` call) drive a real, ASYNC session effect. Idempotent: re-wiring replaces
    /// the channel (a fresh session generation gets a fresh queue).
    pub fn wire_control_channel(&self) {
        let (tx, rx): (UnboundedSender<ControlOp>, UnboundedReceiver<ControlOp>) =
            tokio::sync::mpsc::unbounded_channel();
        self.set_control_sink(Arc::new(move |op| {
            tx.send(op).map_err(|e| format!("control channel closed: {e}"))
        }));
        *Self::lock(&self.control_rx) = Some(rx);
    }

    /// Whether an extension has called `ctx.shutdown()` through this backend, latched at the moment
    /// of the call (Pi's `shutdownHandler`, rpc-mode.ts:344-346). Read by
    /// `AgentSession::shutdown_requested`, which ORs it with the flag the queued-op drain sets, so
    /// the request is visible whether or not a turn boundary ever came round to drain the queue.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Drain every queued control op (non-blocking). The session applies the session-tier ops and
    /// hands the rest to the runtime (Pi `createCommandContext`, agent-session.ts:1158).
    pub fn take_pending_control(&self) -> Vec<ControlOp> {
        let mut g = Self::lock(&self.control_rx);
        let mut out = Vec::new();
        if let Some(rx) = g.as_mut() {
            while let Ok(op) = rx.try_recv() {
                out.push(op);
            }
        }
        out
    }

    /// Attach the running session's tree manager so a guest's state-mutating capabilities
    /// (`append_entry`/`set_session_name`/`set_label`) reach the REAL session tree (arch-08 §5.6).
    /// The builder calls this once the `Arc<AsyncMutex<SessionManager>>` exists (step 10). Also caches
    /// the immutable session id (+ current file) into the sync snapshot for the P-2 `session_id()`/
    /// `session_file()` reads, so a background caller resolves them even while a turn holds the manager
    /// lock. The manager is uncontended at attach time (build), so the non-blocking `try_lock` succeeds.
    pub fn attach_session(&self, manager: Arc<AsyncMutex<SessionManager>>) {
        if let Ok(mgr) = manager.try_lock() {
            let mut snap = Self::lock(&self.snapshot);
            snap.session_id = Some(mgr.session_id().as_str().to_string());
            snap.session_file = mgr.session_file().map(Path::to_path_buf);
        }
        *Self::lock(&self.manager) = Some(manager);
    }

    /// Attach the late-bound message-injection sink (R-SA-101 / P-2). `AgentSession::into_shared` binds
    /// a sink that upgrades a weak self-handle and spawns the async inject/turn, so a background task
    /// calling [`HostServices::inject_message`] reaches THIS session's live turn loop. Idempotent:
    /// re-binding replaces the sink (a fresh session generation gets a fresh handle).
    pub fn set_inject_sink(&self, sink: InjectSink) {
        *Self::lock(&self.inject_sink) = Some(sink);
    }

    /// Share the session's authoritative dynamic-tool view so a guest's `setActiveTools`/
    /// `getActiveTools` capability read+mutates the SAME state the host/CLI tool-toggle does (Pi
    /// `getActiveTools`/`setActiveTools`, agent-session.ts:2281,2283). The builder calls this once the
    /// shared `Arc<Mutex<DynamicToolState>>` exists (step 6), before any guest can be loaded.
    pub(crate) fn attach_dynamic_tools(&self, dynamic_tools: Arc<Mutex<DynamicToolState>>) {
        *Self::lock(&self.dynamic_tools) = Some(dynamic_tools);
    }

    /// Drain the `(tools, system_prompt)` push a guest `setActiveTools` queued;
    /// [`crate::AgentSession::apply_pending_control`] applies it to the live agent before the next
    /// turn (the guest ran the restriction synchronously across the wasm-suspended call).
    pub fn take_pending_active_tools(&self) -> Option<ActiveToolsPush> {
        Self::lock(&self.pending_active_tools).take()
    }

    /// Drain the facade events queued by guest state mutations (entry_appended/session_info_changed);
    /// [`crate::AgentSession::apply_pending_control`] fans them out on the live streams. The guest is
    /// wasm-suspended across the SYNC mutation, so — mirroring the control queue — the ASYNC fan-out
    /// runs at the next command-tier-safe drain.
    pub fn take_pending_events(&self) -> Vec<AgentSessionEvent> {
        std::mem::take(&mut *Self::lock(&self.pending_events))
    }

    /// Acquire the attached manager without blocking (the guest host call runs on the session task
    /// while the manager lock is free — Pi appends synchronously). `Err` (never a panic) when the
    /// session is unattached or transiently busy, surfaced to the guest as a WIT `result` error.
    fn with_manager<R>(
        &self,
        f: impl FnOnce(&mut SessionManager) -> Result<R, String>,
    ) -> Result<R, String> {
        let manager = Self::lock(&self.manager).clone().ok_or("session not attached")?;
        let mut guard = manager.try_lock().map_err(|_| "session busy".to_string())?;
        f(&mut guard)
    }
}

impl HostServices for LiveHostServices {
    // --- ui dialog grant (arch-08 §5.6; Pi `ExtensionUIContext`, types.ts:127-133,216) ---
    // Reaching here means the load-time trust gate already passed (an untrusted extension gets
    // `DenyServices`, whose ui methods return false/None), so like the `exec` grant there is NO extra
    // per-call trust/tier check: ui works at both Event and Command tier, purely a function of whether
    // a mode installed a `ui_sink`. With NO sink (headless print/json) every method falls through to
    // the deny default WITHOUT blocking — byte-for-byte Pi `noOpUIContext` (runner.ts:230-261).

    fn confirm(&self, prompt: &str, message: &str, opts: &DialogOptions) -> bool {
        match self.ui_roundtrip(UiKind::Confirm, prompt, Value::Null, message.to_string(), None, opts) {
            Some(UiReply::Confirm(b)) => b,
            _ => false,
        }
    }

    fn input(&self, prompt: &str, placeholder: Option<&str>, opts: &DialogOptions) -> Option<String> {
        let placeholder = placeholder.map(str::to_string);
        match self.ui_roundtrip(UiKind::Input, prompt, Value::Null, String::new(), placeholder, opts) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn select(&self, prompt: &str, options: &Value, opts: &DialogOptions) -> Option<String> {
        match self.ui_roundtrip(UiKind::Select, prompt, options.clone(), String::new(), None, opts) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn editor(&self, title: &str, initial: &str) -> Option<String> {
        // The WIT `editor(title, initial) -> option<string>` carries no options bag (world.wit:267);
        // use the empty default so the roundtrip signature stays uniform. `title` rides `prompt`
        // (uniform across all four dialog kinds); `initial` rides `message` (mirroring `confirm`'s
        // reuse of the same field for its second string argument).
        match self.ui_roundtrip(
            UiKind::Editor,
            title,
            Value::Null,
            initial.to_string(),
            None,
            &DialogOptions::default(),
        ) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn open_overlay(&self, overlay: Box<dyn InteractiveOverlay>) -> bool {
        // No renderer attached (headless print/json, RPC, or a bare embedder): report "not taken"
        // WITHOUT blocking, so the caller falls back to its own non-interactive surface. This is
        // pi's `if (!ctx.hasUI)` branch, expressed as a return value rather than a capability probe.
        let Some(sink) = Self::lock(&self.overlay_sink).clone() else { return false };
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        if sink.send(OverlayRequest { overlay, done: done_tx }).is_err() {
            // The renderer's run loop is gone — degrade exactly as `ui_roundtrip` does.
            return false;
        }
        // Block this task (NOT the run loop's — every caller of a native command handler is a
        // SPAWNED task, `app.rs`'s `AppAction::Submit`/`ExtensionShortcut` arms) until the renderer
        // tears the modal down. `block_in_place` frees the worker thread meanwhile, the same
        // pattern [`Self::ui_roundtrip`] and the `exec` grant use; both interactive entry points run
        // on the multi-threaded runtime this requires.
        //
        // Deliberately NO timeout: an overlay is a modal the human is looking at, and pi's
        // `await ctx.ui.custom(...)` has no timer either. A dropped sender (renderer gone, session
        // swapped, app quit) resolves `done_rx` as `Err`, which is still a resolution — the block
        // cannot outlive the renderer.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let _ = done_rx.await;
            });
        });
        true
    }

    // --- fire-and-forget ui effects (see [`UiEffect`]'s doc for the Pi citation per variant) ---
    // Unlike `confirm`/`input`/`select`/`editor` above, these never block: [`Self::emit_ui_effect`]
    // sends on an unbounded channel and returns immediately, matching Pi's own `void`-returning
    // `ExtensionUIContext` mutators.

    fn notify(&self, message: &str, kind: NotifyKind) {
        self.emit_ui_effect(UiEffect::Notify { message: message.to_string(), kind });
    }

    fn set_status(&self, key: &str, text: Option<&str>) {
        self.emit_ui_effect(UiEffect::SetStatus { key: key.to_string(), text: text.map(str::to_string) });
    }

    fn set_widget(&self, widget: &Value) {
        self.emit_ui_effect(UiEffect::SetWidget { widget: widget.clone() });
    }

    fn set_header(&self, content: &str) {
        self.emit_ui_effect(UiEffect::SetHeader { content: content.to_string() });
    }

    fn set_footer(&self, content: &str) {
        self.emit_ui_effect(UiEffect::SetFooter { content: content.to_string() });
    }

    fn set_title(&self, title: &str) {
        self.emit_ui_effect(UiEffect::SetTitle { title: title.to_string() });
    }

    fn set_editor_text(&self, text: &str, is_paste: bool) {
        self.emit_ui_effect(UiEffect::SetEditorText { text: text.to_string(), is_paste });
    }

    fn set_tools_expanded(&self, expanded: bool) {
        self.emit_ui_effect(UiEffect::SetToolsExpanded { expanded });
    }

    fn models(&self) -> Value {
        serde_json::to_value(self.provider.models()).unwrap_or_else(|_| json!([]))
    }

    fn current_model(&self) -> Option<String> {
        Self::lock(&self.snapshot)
            .model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider.as_str(), m.model.as_str()))
    }

    fn thinking_level(&self) -> Option<String> {
        Self::lock(&self.snapshot).thinking_level.clone()
    }

    fn context_usage(&self) -> Value {
        let g = Self::lock(&self.snapshot);
        let fraction = if g.context_window == 0 {
            0.0
        } else {
            (g.used_tokens as f64 / g.context_window as f64).clamp(0.0, 1.0)
        };
        json!({
            "usedTokens": g.used_tokens,
            "contextWindow": g.context_window,
            "fraction": fraction,
        })
    }

    fn session_name(&self) -> Option<String> {
        Self::lock(&self.snapshot).session_name.clone()
    }

    fn session_id(&self) -> Option<String> {
        // The immutable session id, cached at `attach_session`. A sync read of the snapshot (no
        // manager lock) so a background caller resolves it even while a turn holds the async lock
        // (P-2; the permission spool routes hard on this id).
        Self::lock(&self.snapshot).session_id.clone()
    }

    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        // C3 (reconciliation §1 / §4 step 6): the ONE session-scoped lock BOTH companions serialize
        // their human prompts on. Every native handed this backend Arc via `set_host_services` reads
        // this SAME instance, so a permission `ask` dialog and an intercom clarify can never prompt the
        // same human simultaneously.
        Some(Arc::clone(&self.human_interaction))
    }

    fn session_file(&self) -> Option<PathBuf> {
        // The LIVE persisted file (deferred until the first assistant message; changes on fork), read
        // from the attached tree manager. `Ok(_)` — attached and read (the value may itself be `None`
        // if not yet persisted); `Err(_)` — unattached OR the lock is momentarily contended (a
        // background caller during an in-progress turn), in which case fall back to the cached
        // snapshot (P-2). Non-blocking `try_lock` (via `with_manager`) — never a hang, never a panic.
        match self.with_manager(|mgr| Ok(mgr.session_file().map(Path::to_path_buf))) {
            Ok(file) => file,
            Err(_) => Self::lock(&self.snapshot).session_file.clone(),
        }
    }

    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        trigger_turn: bool,
    ) -> Result<(), String> {
        // Forward onto the late-bound inject sink (bound by `AgentSession::into_shared`), which spawns
        // the async append/turn on the live session and returns immediately — the sync caller (guest
        // or a native extension's background task) never blocks for the whole turn (R-SA-101 / P-2).
        // No sink (default host / headless-by-value session) ⇒ the seam is unavailable, matching the
        // trait deny default.
        let sink = Self::lock(&self.inject_sink)
            .clone()
            .ok_or("message injection not wired to a live session")?;
        sink(InjectMessage {
            content: content.to_string(),
            custom_type: custom_type.map(str::to_string),
            display,
            trigger_turn,
        })
    }

    fn control(&self, op: ControlOp) -> Result<(), String> {
        // EXT-005: `ctx.abort()` must interrupt the run that is in flight RIGHT NOW. Pi binds it to
        // `void this.abort()` invoked synchronously from the handler (agent-session.ts:2412-2418);
        // cyrup's control queue drains only at turn boundaries, so a queued-only abort would fire
        // after the run it was meant to stop had already ended — aborting nothing. Fire the live
        // interrupt here, then still queue the op so the turn-boundary drain observes it (abort is
        // idempotent, R-11-018) and a host without a live session keeps the queued behavior.
        if matches!(op, ControlOp::Abort)
            && let Some(activity) = Self::lock(&self.activity).clone()
        {
            activity.abort();
        }
        // EXT-005, same rationale one rung further: Pi's `shutdownHandler` sets its flag the instant
        // it is called, with no queue in between. Latch here so a request that arrives while the
        // session is idle — or after the in-flight run's own control drain has already run — is
        // still observable at the host's next settle point instead of stranded in the queue.
        if matches!(op, ControlOp::Shutdown) {
            self.shutdown_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let sink = Self::lock(&self.control).clone();
        match sink {
            Some(f) => f(op),
            None => Err("control capability not yet wired to a runtime".into()),
        }
    }

    // ---- EXT-005 `ctx-state` reads (Pi agent-session.ts:2409-2434) ----------------------------

    fn is_idle(&self) -> bool {
        // Live, never mirrored: a handler asks precisely while a run is in flight.
        match Self::lock(&self.activity).clone() {
            Some(a) => a.is_idle(),
            None => true,
        }
    }

    fn has_pending_messages(&self) -> bool {
        match Self::lock(&self.activity).clone() {
            Some(a) => a.pending_message_count() > 0,
            None => false,
        }
    }

    fn is_project_trusted(&self) -> bool {
        Self::lock(&self.snapshot).project_trusted
    }

    fn system_prompt(&self) -> Option<String> {
        Self::lock(&self.snapshot).system_prompt.clone()
    }

    fn exec(
        &self,
        cmd: &str,
        args: &[String],
        opts: &Value,
        cancel: CancelToken,
    ) -> Result<ExecOutput, String> {
        // The `exec` GRANT (arch-08 §5.6): reaching here means the load-time trust gate already said
        // yes (`is_trusted = origin.is_pre_trust() || project_trusted`, loader.rs:57-60, enforced
        // facade.rs:563) — an untrusted extension gets `DenyServices` and never lands here. So this
        // adds NO extra trust/tier check; it just runs the command, 1:1 with Pi `execCommand`
        // (exec.ts:34-46): shell:false argv, `cwd ?? sessionCwd`, and a `timeoutMs` that SIGTERMs
        // then, after a 5s grace period, SIGKILLs (killed=true) the process GROUP on expiry — Pi's
        // exact `killProcess` escalation (exec.ts:52-63), implemented by `LocalProc::exec_argv`'s
        // SIGTERM/grace/SIGKILL loop (`cyrup-tools/src/ops/local.rs`). Deliberately does NOT honor a
        // guest-supplied `env` key: Pi's real `execCommand` never passes an `env` override to
        // `spawn()` at all (`exec.ts:41-45`) — the child only ever inherits the host's own ambient
        // environment. Accepting one here would be new ambient authority (arbitrary env injection
        // for a spawned process) with no Pi equivalent (`cyrup-ext-sdk::descriptor::ExecOptions` has
        // no `env` field for exactly this reason) — do not re-add without a real Pi citation.
        // A guest-supplied `cwd: ""` must fall back to the session cwd exactly like an omitted
        // `cwd` does (`.filter` below), not short-circuit `unwrap_or_else` with an empty override.
        // Pi's real `ctx.exec` (`loader.ts:319`: `execCommand(command, args, options?.cwd ?? cwd,
        // options)`) only falls back on `undefined`/`null` via `??` — a literal `""` stays `""` all
        // the way to Node's `child_process.spawn({cwd:""})`, which treats a FALSY cwd as "no
        // override" and inherits the parent's own ambient cwd (verified live) instead of erroring.
        // cyrup's `self.cwd` (the session's project directory) is the analog of that ambient
        // fallback here (same reasoning `HostServices::proc_spawn`'s omitted-cwd default already
        // documents), so an empty guest string gets the SAME fallback an omitted one gets, rather
        // than reaching `LocalProc::exec_argv`'s unconditional `cmd.current_dir(cwd)`
        // (`cyrup-tools/src/ops/local.rs`) with an empty path and hard-failing the spawn.
        let cwd = opts
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cwd.clone());
        // L4 review: falls back to [`DEFAULT_EXEC_TIMEOUT`] (`self.exec_timeout`) when the guest gave
        // no `timeoutMs` (or gave `0`) — see that constant's doc for why an unbounded `exec` here,
        // unlike Pi's own unbounded `execCommand`, has no live abort escape hatch to fall back on.
        let timeout = opts
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis)
            .or(Some(self.exec_timeout));
        let spec =
            ArgvSpec { program: cmd.to_string(), args: args.to_vec(), cwd, env: Vec::new() };
        let proc = self.proc.clone();
        // The `HostServices` trait is sync (the guest is wasm-suspended across the call); drive the
        // async process ops to completion on the current multi-threaded runtime worker.
        let outcome = tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(proc.exec_argv(spec, cancel, timeout))
        });
        let out = match outcome {
            Ok(o) => o,
            // Pi `execCommand` never rejects: a spawn/wait failure resolves `{code:1}` (exec.ts:99-105).
            Err(_) => {
                return Ok(ExecOutput { code: 1, stdout: String::new(), stderr: String::new(), killed: false });
            }
        };
        // Map onto Pi's `{code, killed}` (exec.ts:49,97; `child-process.ts:73-80`): `killed` is set
        // the instant a SIGTERM/SIGKILL escalation is INITIATED and is completely orthogonal to
        // `code` — a process that catches SIGTERM and exits itself mid-grace still reports its REAL
        // exit code, `killed` never masks it. `out.killed`/`out.status` already preserve exactly that
        // split (`LocalProc::exec_argv`); do not re-derive `killed` from the status variant.
        let killed = out.killed;
        let code = match out.status {
            ExitStatus::Exited(n) => n,
            ExitStatus::Signaled | ExitStatus::Killed | ExitStatus::TimedOut => 0,
        };
        Ok(ExecOutput {
            code,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            killed,
        })
    }

    // --- http-client GRANT (arch-08 §3.2 draft; pi-mcp-adapter-port.md §3.2) ---
    // Reaching here means the load-time trust gate already said yes (the SAME gate `exec` uses,
    // `is_trusted = origin.is_pre_trust() || project_trusted`, loader.rs:57-60) — an untrusted
    // extension gets `DenyServices` and never lands here. So, like `exec`, this adds NO extra
    // trust/tier check or per-host allowlist; it just runs the request through the real `HttpCaps`
    // engine. The `HostServices` trait is sync (the guest is wasm-suspended across the call); drive
    // the async `reqwest` calls to completion on the current multi-threaded runtime worker — the SAME
    // `block_in_place` + `block_on` bridge the `exec` grant uses.

    fn http_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.http.request(req))
        })
    }

    fn http_request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.http.request_stream(req))
        })
    }

    fn http_poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.http.poll_stream_chunk(handle))
        })
    }

    fn http_close_stream(&self, handle: u32) {
        self.http.close_stream(handle);
    }

    // --- proc GRANT (arch-08 §5.2 request/poll bridge; pi-mcp-adapter-port.md §3.1) ---
    // Reaching here means the load-time trust gate already said yes (the SAME gate `exec`/
    // `http-client` use) — no extra trust/tier check. `spawn`/`read_stdout`/`read_stderr`/
    // `poll_exit` are sync (no `.await` in `ProcCaps` for those); `write_stdin`/`kill` need to
    // `.await` real pipe I/O / process-termination confirmation, driven to completion via the SAME
    // `block_in_place` + `block_on` bridge `exec`/`http_request` use.

    fn proc_spawn(&self, spec: &ProcSpawnSpec) -> Result<u32, String> {
        // Default an omitted `cwd` to the session's own project directory — the SAME fallback
        // `exec` applies (`opts.cwd ?? self.cwd` above), for the SAME reason: the real consumer's
        // own default (`server-manager.ts:110`'s `resolveConfigPath(definition.cwd)`, which is
        // `undefined` when `definition.cwd` is `undefined`, `utils.ts:78-80`) relies on ITS
        // coordinating process's OWN ambient `process.cwd()` reliably already BEING the project
        // directory, since pi-mcp-adapter runs as part of the per-invocation coding-agent process
        // rooted there. `cyrup-session-svc` is architected as a long-lived MULTI-session service
        // with an explicit per-session `cwd` field precisely because the ambient host-process cwd
        // is NOT a reliable stand-in for a given session's project directory here — so, unlike Pi,
        // omitting `cwd` must not silently fall through to `tokio::process::Command`'s own default
        // (inheriting the HOST's ambient cwd, not the calling session's), or a guest-authored
        // MCP-client extension that (correctly, matching Pi) omits `cwd` could spawn the server in
        // the wrong directory under concurrent multi-session deployment.
        let spec = if spec.cwd.is_none() {
            ProcSpawnSpec { cwd: Some(self.cwd.clone()), ..spec.clone() }
        } else {
            spec.clone()
        };
        self.proc_caps.spawn(&spec)
    }

    fn proc_write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.proc_caps.write_stdin(handle, data))
        })
    }

    fn proc_read_stdout(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.proc_caps.read_stdout(handle, max_bytes)
    }

    fn proc_read_stderr(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.proc_caps.read_stderr(handle, max_bytes)
    }

    fn proc_poll_exit(&self, handle: u32) -> Option<i32> {
        self.proc_caps.poll_exit(handle)
    }

    fn proc_kill(&self, handle: u32) -> Result<(), String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.proc_caps.kill(handle))
        })
    }

    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        // Persist the custom (non-LLM) entry into the LIVE session tree (Pi
        // `sessionManager.appendCustomEntry`, agent-session.ts:2265-2271) and snapshot the persisted
        // entry for the `entry_appended` fan-out.
        let (id, entry) = self.with_manager(|mgr| {
            let id = mgr.append_custom_entry(custom_type, Some(data.clone())).map_err(|e| e.to_string())?;
            let entry = mgr.entry(&id).and_then(|e| serde_json::to_value(e).ok()).unwrap_or(Value::Null);
            Ok((id, entry))
        })?;
        Self::lock(&self.pending_events).push(AgentSessionEvent::EntryAppended { entry });
        Ok(id.to_string())
    }

    fn set_session_name(&self, name: &str) {
        // Rename the live session (Pi `setSessionName` → `appendSessionInfo`, agent-session.ts:2690).
        let resolved = self.with_manager(|mgr| {
            mgr.append_session_info(name).map_err(|e| e.to_string())?;
            Ok(mgr.session_name())
        });
        if let Ok(resolved) = resolved {
            // Keep the sync read-view snapshot current (guest `getSessionName` reflects the rename)
            // and queue the `session_info_changed` fan-out (Pi `_emit`, agent-session.ts:2714).
            Self::lock(&self.snapshot).session_name = resolved.clone();
            Self::lock(&self.pending_events)
                .push(AgentSessionEvent::SessionInfoChanged { name: resolved });
        }
    }

    fn set_label(&self, entry_id: &str, label: Option<&str>) {
        // Set/replace the entry's label on the live tree (Pi `setLabel` → `appendLabel`,
        // agent-session.ts:2276-2279). `None` CLEARS the label — pi's `setLabel(entryId, undefined)`
        // — which is why the capability's parameter is optional. A no-op result (unknown id / busy)
        // degrades silently.
        let _ = self.with_manager(|mgr| {
            mgr.append_label(&EntryId::from(entry_id), label).map_err(|e| e.to_string())?;
            Ok(())
        });
    }

    fn active_tools(&self) -> Option<Vec<String>> {
        // The live session's REAL active tool set (Pi `getActiveTools` = `getActiveToolNames`,
        // agent-session.ts:2281,813). `None` until the shared view is attached (default host).
        let dt = Self::lock(&self.dynamic_tools).clone()?;
        Some(Self::lock(&dt).active_names())
    }

    fn all_tool_names(&self) -> Option<Vec<String>> {
        // The live session's FULL registered tool set (Pi `getAllTools`, agent-session.ts:790-799) —
        // the whole enable-able `_toolRegistry`, NOT the exposed subset `active_tools` returns. This is
        // the `getAllTools` analog the permission companion's registry / unknown-tool gate checks
        // against (pi-permission-system index.ts:2218-2228). `None` until the shared dynamic-tool view
        // is attached (default host: no live agent → the companion skips the registry gate).
        let dt = Self::lock(&self.dynamic_tools).clone()?;
        Some(Self::lock(&dt).all().into_iter().map(|t| t.name).collect())
    }

    fn set_active_tools(&self, names: &[String]) {
        // Restrict the live agent's tool set (Pi `setActiveTools` = `setActiveToolsByName`,
        // agent-session.ts:2283,840-855). Update the authoritative dynamic-tool view SYNCHRONOUSLY —
        // Pi mutates `this.agent.state.tools` immediately, so the paired `getActiveTools` read
        // reflects it at once — and queue the rebuilt `(tools, prompt)` for the ASYNC agent push
        // `AgentSession::apply_pending_control` applies before the next turn (the guest is
        // wasm-suspended across this SYNC call, the same sync→async bridge control ops use). No-op
        // when no shared view is attached (default host: no live agent to restrict).
        let Some(dt) = Self::lock(&self.dynamic_tools).clone() else { return };
        let (tools, prompt) = { Self::lock(&dt).set_active(names) };
        *Self::lock(&self.pending_active_tools) = Some((tools, prompt));
    }
}

/// The `getActiveTools` source the registered-tool wrapper diffs around every `execute` (Pi binds
/// `runtime.getActiveTools` from the session's own actions, extensions/runner.ts:330, and
/// `wrapRegisteredTool` calls it either side of the tool, extensions/wrapper.ts:23-25).
///
/// Deliberately the SAME read `HostServices::active_tools` performs — the authoritative
/// `DynamicToolState`, which `set_active_tools` mutates SYNCHRONOUSLY. That synchronicity is what
/// makes the diff observable: a tool that calls `setActiveTools` during its own `execute` has
/// already widened this view by the time the wrapper takes its "after" snapshot.
impl cyrup_ext::ActiveToolNames for LiveHostServices {
    fn active_tool_names(&self) -> Option<Vec<String>> {
        let dt = Self::lock(&self.dynamic_tools).clone()?;
        Some(Self::lock(&dt).active_names())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_provider::faux::FauxProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A backend seeded with the real local process ops + a temp cwd (the `exec` grant path).
    fn svc_with(provider: Arc<dyn Provider>) -> LiveHostServices {
        LiveHostServices::new(provider, cyrup_tools::Backend::default().proc, std::env::temp_dir())
    }

    #[test]
    fn reflects_live_model_and_models_catalog() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider.clone());

        // Before wiring: no current model, control denied, but the catalog is live from the provider.
        assert!(svc.current_model().is_none());
        assert!(svc.control(ControlOp::Reload).is_err());
        let models = svc.models();
        assert!(models.is_array(), "models() must serialize the provider catalog");
        assert!(!models.as_array().unwrap().is_empty(), "faux provider has at least one model");

        // After the session pushes its active model, the read reflects it.
        let m = ModelRef { provider: "faux".into(), api: None, model: "faux-1".into() };
        svc.update_model(m, 128_000, Some("medium".into()));
        svc.update_state(Some("my session".into()), 42);
        assert_eq!(svc.current_model().as_deref(), Some("faux/faux-1"));
        assert_eq!(svc.thinking_level().as_deref(), Some("medium"));
        assert_eq!(svc.session_name().as_deref(), Some("my session"));
        let usage = svc.context_usage();
        assert_eq!(usage["usedTokens"], json!(42));
        assert_eq!(usage["contextWindow"], json!(128_000));
    }

    #[test]
    fn control_routes_to_the_wired_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        svc.set_control_sink(Arc::new(move |_op| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        svc.control(ControlOp::Reload).expect("control routes to the sink");
        svc.control(ControlOp::Compact { custom_instructions: None }).expect("control routes to the sink");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// The `exec` grant runs a DIRECT argv (shell:false) command and returns the REAL captured
    /// output/code/killed — 1:1 with Pi `execCommand` (exec.ts:34-46). Multi-thread runtime so the
    /// sync grant can `block_in_place` on the async process ops.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_runs_argv_with_cwd_env_and_reports_killed_on_timeout() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // 1) Real stdout + exit code, NO shell (argv `echo hi`).
        let out = svc
            .exec("echo", &["hi".to_string()], &json!({}), CancelToken::new())
            .expect("echo runs via the exec grant");
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.code, 0);
        assert!(!out.killed, "a natural exit is not `killed`");

        // 2) shell:false — an argv that a shell would splice is passed literally, so `echo` prints the
        //    metacharacters verbatim (proves no `bash -c` word-splitting).
        let out = svc
            .exec("echo", &["a; echo b".to_string()], &json!({}), CancelToken::new())
            .expect("echo runs");
        assert_eq!(out.stdout, "a; echo b\n", "argv is literal — no shell interpretation");

        // 3) `cwd` option honored (Pi `opts?.cwd ?? cwd`).
        let tmp = std::env::temp_dir();
        let out = svc
            .exec("pwd", &[], &json!({ "cwd": tmp.to_string_lossy() }), CancelToken::new())
            .expect("pwd runs");
        let printed = std::fs::canonicalize(out.stdout.trim_end()).unwrap_or_default();
        assert_eq!(printed, std::fs::canonicalize(&tmp).unwrap_or(tmp), "exec ran in the given cwd");

        // 4) a guest-supplied `env` key is IGNORED — Pi's real `execCommand` (exec.ts:41-45) never
        //    accepts an env override at all; the child only inherits the host's own ambient
        //    environment (Node `spawn()`'s default when no `env` key is passed). If the `exec` grant
        //    honored a guest's `env`, `printenv` would see the injected value; instead the lookup
        //    variable must be genuinely UNSET in the child (nonzero exit, empty stdout) — proving
        //    this is NOT new ambient authority beyond Pi's real surface.
        let out = svc
            .exec(
                "printenv",
                &["CYRUP_EXEC_TEST_ENV_MUST_BE_IGNORED".to_string()],
                &json!({ "env": { "CYRUP_EXEC_TEST_ENV_MUST_BE_IGNORED": "injected" } }),
                CancelToken::new(),
            )
            .expect("printenv runs (even though the variable it looks up is unset)");
        assert_ne!(
            out.code, 0,
            "a guest-supplied `env` override must be ignored — printenv must NOT find an injected \
             value"
        );
        assert!(out.stdout.is_empty(), "no injected value may ever reach the child's environment");

        // 5) `timeoutMs` ⇒ the host SIGTERMs the group, then (since `sleep` obeys SIGTERM and dies
        //    well within the 5s grace period, no SIGKILL escalation needed here) reports
        //    `killed=true` (Pi `killProcess` sets `killed`, exec.ts:52-63).
        let out = svc
            .exec("sleep", &["30".to_string()], &json!({ "timeoutMs": 100 }), CancelToken::new())
            .expect("sleep runs then is killed on timeout");
        assert!(out.killed, "a timed-out exec is `killed`");

        // 6) an already-aborted signal (pre-cancelled token) kills immediately ⇒ `killed=true`.
        let cancelled = CancelToken::new();
        cancelled.cancel();
        let out = svc
            .exec("sleep", &["30".to_string()], &json!({}), cancelled)
            .expect("a pre-cancelled exec resolves");
        assert!(out.killed, "a pre-aborted signal kills the exec");

        // 7) a well-behaved child that TRAPS SIGTERM and exits itself with its OWN real code must
        //    have that REAL code surfaced through the grant end-to-end — `killed` is orthogonal,
        //    never masking it — 1:1 with Pi's `{code, killed}` (`exec.ts:97`; `child-process.ts:73-
        //    80`'s `finalize(exitCode)` always carries the real observed code).
        let out = svc
            .exec(
                "sh",
                &["-c".to_string(), "trap 'exit 7' TERM; while true; do sleep 1; done".to_string()],
                &json!({ "timeoutMs": 100 }),
                CancelToken::new(),
            )
            .expect("the SIGTERM-trapping child runs then exits itself");
        assert_eq!(out.code, 7, "the child's own real exit code survives a host-initiated kill");
        assert!(out.killed, "a timeout-initiated kill is still `killed`, independent of `code`");
    }

    /// L4 round-12 finding #3: `exec`'s `cwd` option must treat a guest-supplied EMPTY string the
    /// same as an OMITTED one — falling back to the session cwd — not short-circuit
    /// `unwrap_or_else` with an empty override. Pi's real `ctx.exec` (`loader.ts:319`:
    /// `options?.cwd ?? cwd`) only falls back via `??` on `undefined`/`null`; a literal `""` stays
    /// `""` all the way to Node's `child_process.spawn({cwd:""})`, which (verified live) treats a
    /// FALSY cwd as "no override" and inherits the parent's ambient cwd rather than erroring —
    /// `self.cwd` (the session's project directory) is the cyrup-analog of that ambient fallback.
    /// Verified by actually running `pwd` inside the spawned child and reading its REAL stdout +
    /// exit code (pre-fix: `std::process::Command::current_dir("")` hard-fails the spawn, which
    /// `exec`'s `Err(_) => Ok(ExecOutput{code:1,..})` mapping — Pi's `execCommand` never rejects,
    /// exec.ts:99-105 — turned into a SILENT `code:1`/empty-stdout failure instead of running in the
    /// session cwd).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_treats_an_empty_guest_cwd_the_same_as_omitted() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let session_cwd = std::env::temp_dir();
        let svc = svc_with(provider);

        let out = svc
            .exec("pwd", &[], &json!({ "cwd": "" }), CancelToken::new())
            .expect("pwd runs even though the guest passed an empty cwd");
        assert_eq!(out.code, 0, "must NOT silently degrade to code:1 the way a hard current_dir(\"\") spawn failure would");
        let printed = std::fs::canonicalize(out.stdout.trim_end()).unwrap_or_default();
        assert_eq!(
            printed,
            std::fs::canonicalize(&session_cwd).unwrap_or(session_cwd),
            "an empty guest cwd must fall back to the SESSION's cwd, exactly like an omitted one"
        );
    }

    /// L4 review: `exec` must never be truly UNBOUNDED when the guest gives no `timeoutMs` (or `0`) —
    /// unlike Pi's own `execCommand` (exec.ts:74-79), which is also unbounded absent a `timeout` but
    /// can still be interrupted live via a real `AbortSignal` listener (exec.ts:65-72), cyrup's `exec`
    /// grant blocks the guest wasm-suspended for the ENTIRE synchronous host call — a `signalId` can
    /// only pre-cancel an ALREADY-aborted signal at call entry, never mid-run — so an untimed call has
    /// no live escape hatch at all without [`DEFAULT_EXEC_TIMEOUT`]. Proven with a REAL never-exiting
    /// child (`sleep 3600`) and NO `timeoutMs` in `opts` at all, against a tiny overridden fallback
    /// (`with_exec_timeout`) so the test doesn't wait the full 120s production ceiling: the exec call
    /// must still return promptly, `killed`, via the SAME SIGTERM-then-grace-then-SIGKILL escalation
    /// an explicit guest timeout gets (proving the fallback is fed into `exec_argv`'s real `timeout`
    /// parameter, not merely abandoning/leaking the child via an outer future-drop).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_with_no_timeout_ms_still_gets_killed_by_the_fallback_ceiling() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = LiveHostServices::with_exec_timeout(
            provider,
            cyrup_tools::Backend::default().proc,
            std::env::temp_dir(),
            Duration::from_millis(100),
        );

        let started = std::time::Instant::now();
        let out = svc
            .exec("sleep", &["3600".to_string()], &json!({}), CancelToken::new())
            .expect("exec resolves even though the guest gave no timeoutMs at all");
        let elapsed = started.elapsed();

        assert!(out.killed, "the fallback ceiling must kill an untimed exec — a 3600s sleep can never exit on its own");
        assert!(
            elapsed < Duration::from_secs(10),
            "must be bounded by the (overridden, 100ms) fallback ceiling, not the real 3600s sleep — \
             took {elapsed:?}"
        );
    }

    /// The `proc` grant's `spawn` defaults an OMITTED `cwd` to the session's own project directory —
    /// the SAME fallback `exec` applies (test 3 above, `opts.cwd ?? self.cwd`) — rather than
    /// silently inheriting the HOST PROCESS's own ambient cwd (`tokio::process::Command`'s default
    /// when no `.current_dir()` call is made at all). Verified by actually running `pwd` inside the
    /// spawned child and reading its REAL stdout, not asserting on `ProcSpawnSpec` construction.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proc_spawn_defaults_omitted_cwd_to_the_session_cwd() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        let session_cwd = std::env::temp_dir();

        // No `cwd` in the spec at all — must run in the SESSION's cwd, not the host's ambient one
        // (this test binary's own cwd is the crate root, which must NOT be what `pwd` prints).
        let spec = ProcSpawnSpec {
            cmd: "pwd".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            capture_stderr: false,
        };
        let handle = svc.proc_spawn(&spec).expect("pwd spawns with no cwd override");
        let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
        let printed = std::fs::canonicalize(stdout.trim_end()).unwrap_or_default();
        assert_eq!(
            printed,
            std::fs::canonicalize(&session_cwd).unwrap_or(session_cwd),
            "an omitted cwd must default to the SESSION's cwd, not the host process's ambient one"
        );

        // An EXPLICIT `cwd` in the spec is still honored verbatim (the fallback only fires when
        // `cwd` is `None`, never overriding a guest-supplied value).
        let explicit = std::env::current_dir().expect("host has a cwd");
        let spec = ProcSpawnSpec {
            cmd: "pwd".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: Some(explicit.clone()),
            capture_stderr: false,
        };
        let handle = svc.proc_spawn(&spec).expect("pwd spawns with an explicit cwd");
        let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
        let printed = std::fs::canonicalize(stdout.trim_end()).unwrap_or_default();
        assert_eq!(
            printed,
            std::fs::canonicalize(&explicit).unwrap_or(explicit),
            "an explicit cwd is honored verbatim, not overridden by the session-cwd fallback"
        );
    }

    /// Regression test: the session's OWN host-injected default `cwd` (the fallback `proc_spawn`
    /// applies when a guest omits `cwd` entirely, above) must reach the real child VERBATIM, never
    /// re-interpolated — even when that literal project-directory path happens to contain a
    /// `${VAR}`-shaped substring. Before the fix (`caps/proc.rs`'s `ProcCaps::spawn` used to run
    /// EVERY `Some(cwd)` — guest-supplied or host-injected — through `resolve_config_path`), this
    /// spawn call failed outright with ENOENT: interpolating the unset `${MY_REPRO_VAR}` down to an
    /// empty string produced a directory that doesn't exist on disk (only the literal
    /// `${MY_REPRO_VAR}`-named one, created below, does). Verified live: actually spawns `pwd`
    /// through the real `proc_spawn` grant and reads its real stdout.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proc_spawn_never_reinterpolates_the_host_injected_default_cwd() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let base = std::env::temp_dir();
        let weird = base.join("cyrup-session-cwd-${MY_REPRO_VAR}-dir");
        std::fs::create_dir_all(&weird).expect("create the literal, unusual session cwd");
        let svc = LiveHostServices::new(provider, cyrup_tools::Backend::default().proc, weird.clone());

        let spec = ProcSpawnSpec {
            cmd: "pwd".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            capture_stderr: false,
        };
        let handle = svc
            .proc_spawn(&spec)
            .expect("pwd must spawn successfully in the session's literal cwd, not ENOENT");
        let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
        assert_eq!(
            std::fs::canonicalize(stdout.trim_end()).unwrap_or_default(),
            std::fs::canonicalize(&weird).unwrap_or(weird),
            "the host-injected default cwd must survive byte-for-byte, not have ${{MY_REPRO_VAR}} \
             interpolated out of it"
        );
    }

    /// Poll `proc_poll_exit` until the child reaps, then drain its real stdout — used by tests that
    /// need a spawned child's actual captured output rather than just an `Ok` handle.
    #[cfg(unix)]
    async fn wait_for_exit_and_read_stdout(svc: &LiveHostServices, handle: u32) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if svc.proc_poll_exit(handle).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let bytes = svc.proc_read_stdout(handle, 65536).expect("read_stdout on a live handle");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// With NO ui sink attached (headless print/json: `set_ui_sink` is never called), the ui grant
    /// falls through to the trait deny defaults WITHOUT blocking — byte-for-byte Pi `noOpUIContext`
    /// (confirm=false, input/select/editor=None). A single-thread runtime proves it never touches
    /// `block_in_place` (which would panic here) on the headless path.
    #[test]
    fn headless_ui_returns_deny_defaults_without_a_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        assert!(!svc.confirm("ok?", "body", &DialogOptions::default()));
        assert_eq!(svc.input("name?", Some("placeholder"), &DialogOptions::default()), None);
        assert_eq!(svc.select("pick", &json!(["a", "b"]), &DialogOptions::default()), None);
        assert_eq!(svc.editor("title", "seed"), None);
    }

    /// The ui GRANT round-trips a dialog through a scripted [`UiSink`] renderer: the guest-facing
    /// (sync) `confirm`/`input`/`select`/`editor` block on a one-shot while a concurrent responder
    /// answers each [`UiRequest`], exactly as the interactive TUI selector / RPC round-trip does at
    /// runtime. Multi-thread so the `block_in_place` + `block_on` reply-wait is legal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_round_trips_through_a_scripted_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);

        // L4 review §2.6/§2.7 live proof: capture each request's `message`/`placeholder` as the
        // scripted renderer sees them, so the test can assert they arrived distinct from `prompt`.
        #[derive(Clone, Debug)]
        struct Seen {
            kind: UiKind,
            prompt: String,
            message: String,
            placeholder: Option<String>,
        }
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        // The scripted renderer: reply to each request by kind (like a user picking in the selector).
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                seen2.lock().unwrap_or_else(|e| e.into_inner()).push(Seen {
                    kind: req.kind,
                    prompt: req.prompt.clone(),
                    message: req.message.clone(),
                    placeholder: req.placeholder.clone(),
                });
                let reply = match req.kind {
                    UiKind::Confirm => UiReply::Confirm(true),
                    UiKind::Input => UiReply::Text(Some(format!("answer:{}", req.prompt))),
                    UiKind::Select => {
                        // Echo back the LAST option string as the chosen value proof.
                        let chosen = req
                            .options
                            .as_array()
                            .and_then(|a| a.last())
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        UiReply::Text(chosen)
                    }
                    // Echo the seed text (Pi `prefill`, now `req.message` — L4 review §2, editor
                    // title fix): proves `editor`'s two strings arrive on distinct fields, not both
                    // squashed onto `prompt`.
                    UiKind::Editor => UiReply::Text(Some(format!("edited:{}", req.message))),
                };
                let _ = req.reply.send(reply);
            }
        });

        // Each guest-facing call blocks until the responder answers (run on a blocking-capable worker).
        let s1 = svc.clone();
        let confirm = tokio::task::spawn_blocking(move || {
            s1.confirm("proceed?", "a large formatted body, distinct from the title", &DialogOptions::default())
        })
        .await
        .expect("confirm task");
        assert!(confirm, "confirm round-trips the scripted `true`");

        let s2 = svc.clone();
        let input = tokio::task::spawn_blocking(move || {
            s2.input("name?", Some("e.g. Ada Lovelace"), &DialogOptions::default())
        })
        .await
        .expect("input task");
        assert_eq!(input.as_deref(), Some("answer:name?"));

        // §2.6: the confirm `message` reached the renderer verbatim, distinct from `prompt` (title).
        // §2.7: the input `placeholder` reached the renderer verbatim (`Some`, not dropped).
        let seen_snapshot = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            seen_snapshot
                .iter()
                .find(|s| s.kind == UiKind::Confirm)
                .map(|s| (s.prompt.as_str(), s.message.as_str())),
            Some(("proceed?", "a large formatted body, distinct from the title")),
            "confirm's message body round-trips separately from its title: {seen_snapshot:?}"
        );
        assert_eq!(
            seen_snapshot.iter().find(|s| s.kind == UiKind::Input).map(|s| s.placeholder.clone()),
            Some(Some("e.g. Ada Lovelace".to_string())),
            "input's placeholder round-trips instead of being dropped: {seen_snapshot:?}"
        );

        let s3 = svc.clone();
        let select = tokio::task::spawn_blocking(move || {
            s3.select("pick one", &json!(["x", "y", "z"]), &DialogOptions::default())
        })
        .await
        .expect("select task");
        assert_eq!(
            select.as_deref(),
            Some("z"),
            "select returns the chosen option STRING (Pi types.ts:127, world.wit:259)"
        );

        let s4 = svc.clone();
        let editor = tokio::task::spawn_blocking(move || s4.editor("edit this file", "hello"))
            .await
            .expect("editor task");
        assert_eq!(editor.as_deref(), Some("edited:hello"));

        // L4 review §2 (editor title fix) live proof: `editor`'s title reached the renderer on
        // `prompt`, distinct from its seed text on `message` — mirrors the confirm/input assertions
        // above, closing the same class of dropped-field bug for `editor`.
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            seen.iter()
                .find(|s| s.kind == UiKind::Editor)
                .map(|s| (s.prompt.as_str(), s.message.as_str())),
            Some(("edit this file", "hello")),
            "editor's title round-trips separately from its seed text: {seen:?}"
        );
    }

    /// L4 review §2.2: a dialog whose renderer NEVER answers still resolves within `opts.timeout_ms` —
    /// Pi's `createDialogPromise` host-armed `setTimeout(() => resolve(defaultValue), opts.timeout)`
    /// (`rpc-mode.ts:114-119`) ALWAYS settles the awaited Promise regardless of client behavior. The
    /// scripted renderer here receives every request and drops it on the floor (never replies), proving
    /// `ui_roundtrip` races the reply against a REAL timer rather than blocking forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_honors_timeout_ms_and_resolves_to_the_default_on_no_response() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        // The "hung client": receives every request and HOLDS it (keeping `req.reply` open, exactly
        // like the RPC loop's `pending` map keeps a live entry) but never sends a reply — the real
        // shape of a non-responding client, as opposed to a dropped sender (which would resolve the
        // receiver immediately with an error and prove nothing about the timeout race).
        let held: Arc<Mutex<Vec<UiRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let held2 = held.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                held2.lock().unwrap_or_else(|e| e.into_inner()).push(req);
            }
        });

        let opts = DialogOptions { timeout_ms: Some(50), signal_id: None };

        let s1 = svc.clone();
        let o1 = opts.clone();
        let started = tokio::time::Instant::now();
        let confirm = tokio::task::spawn_blocking(move || s1.confirm("proceed?", "body", &o1))
            .await
            .expect("confirm task");
        let elapsed = started.elapsed();
        assert!(!confirm, "an unanswered confirm resolves to Pi's `false` default, not a hang");
        assert!(
            elapsed < Duration::from_secs(2),
            "confirm must settle close to the 50ms timeout, not hang indefinitely (took {elapsed:?})"
        );

        let s2 = svc.clone();
        let o2 = opts.clone();
        let input = tokio::task::spawn_blocking(move || s2.input("name?", Some("placeholder"), &o2))
            .await
            .expect("input task");
        assert_eq!(input, None, "an unanswered input resolves to Pi's `undefined` default");

        let s3 = svc.clone();
        let o3 = opts;
        let select = tokio::task::spawn_blocking(move || s3.select("pick", &json!(["a", "b"]), &o3))
            .await
            .expect("select task");
        assert_eq!(select, None, "an unanswered select resolves to Pi's `undefined` default");
    }

    /// `timeout_ms: 0` means NO timeout, not an instant one — Pi's `createDialogPromise` only arms
    /// its `setTimeout` `if (opts?.timeout)` (`rpc-mode.ts:114`; falsy-zero ⇒ no timer at all). Proven
    /// here the same way the honors-timeout test proves the OPPOSITE: a REAL (delayed, non-default)
    /// reply arrives well after `Duration::from_millis(0)` would already have elapsed under the old
    /// unconditional `.map(Duration::from_millis)` — if `0` were mistakenly armed as a real timer, the
    /// race would resolve to the default (`false`) near-instantly and NEVER see this later reply.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_timeout_ms_zero_means_no_timeout_not_an_instant_one() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                // A REAL answer, deliberately delayed well past when a (bugged) 0ms timer would have
                // already fired and resolved the call to the default.
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = req.reply.send(UiReply::Confirm(true));
            }
        });

        let opts = DialogOptions { timeout_ms: Some(0), signal_id: None };
        let started = tokio::time::Instant::now();
        let confirm = tokio::task::spawn_blocking(move || svc.confirm("proceed?", "body", &opts))
            .await
            .expect("confirm task");
        let elapsed = started.elapsed();

        assert!(
            confirm,
            "timeout_ms:0 must wait for the REAL reply (true), not short-circuit to the `false` \
             default the way a genuine 0ms timeout would"
        );
        assert!(
            elapsed >= Duration::from_millis(120),
            "the call must have actually WAITED for the delayed reply, not resolved near-instantly \
             to the default (took {elapsed:?}, expected >= ~150ms)"
        );
    }

    /// L4 review §2.5 (the shared mechanism half): a reply sent on the SAME one-shot `ui_roundtrip` is
    /// waiting on unblocks it immediately, well before a long `timeout_ms` would otherwise elapse. This
    /// is exactly what the RPC loop's `force_resolve_pending` (`rpc.rs`, wired to `abort`/`abort_retry`)
    /// does to LIVE-dismiss an already-open dialog — no separate cancellation channel is needed because
    /// forcing the existing reply is sufficient, and this proves that path is genuinely live, not merely
    /// a pre-flight snapshot check.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_force_resolved_reply_unblocks_before_a_long_timeout_elapses() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        // Simulate a live "abort": as soon as the dialog opens, force-resolve it directly (the same
        // action `force_resolve_pending` takes) instead of waiting for a real user response.
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.reply.send(UiReply::Confirm(false));
            }
        });

        // A 10-second timeout that must NOT be what unblocks this call.
        let opts = DialogOptions { timeout_ms: Some(10_000), signal_id: None };
        let started = tokio::time::Instant::now();
        let confirm = tokio::task::spawn_blocking(move || svc.confirm("proceed?", "body", &opts))
            .await
            .expect("confirm task");
        let elapsed = started.elapsed();
        assert!(!confirm);
        assert!(
            elapsed < Duration::from_secs(2),
            "a force-resolved reply must win the race immediately, not wait out the 10s timeout (took {elapsed:?})"
        );
    }

    /// The DEFAULT (deny-all) backend denies exec with Pi's "not granted" message — the untrusted
    /// analog (an untrusted extension gets `DenyServices`, arch-08 §5.6).
    #[test]
    fn deny_services_refuses_exec() {
        use cyrup_ext::host::{DenyServices, HostServices as _};
        let err = DenyServices
            .exec("echo", &["hi".to_string()], &json!({}), CancelToken::new())
            .expect_err("deny-all backend refuses exec");
        assert!(err.contains("not granted"), "denied with the Pi message: {err}");
    }
}

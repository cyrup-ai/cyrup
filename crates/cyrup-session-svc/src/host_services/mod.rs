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
//!
//! Layout: this file keeps the backend — [`LiveSnapshot`], [`LiveHostServices`], its inherent
//! `impl` and the single `impl HostServices` block, whose nine grant banners are the seam's real
//! table of contents. The carriers live in [`ui`], [`attach`], [`json`] and [`inject`], re-exported
//! below at the paths `lib.rs` and the rest of the crate already name them by; the tests moved to
//! `src/tests/host_services_*.rs`, into the crate's one test binary.

mod attach;
mod inject;
mod json;
mod ui;

pub use attach::ThemeAccess;
pub use inject::{ControlSink, InjectMessage, InjectSink};
pub use ui::{
    EditorTextMirror, OverlayRequest, OverlaySink, UiEffect, UiEffectSink, UiKind, UiReply,
    UiRequest, UiSink,
};
pub(crate) use attach::{SessionActivity, SessionCatalog};
pub(crate) use json::{builtin_tool_source_info, tree_node_to_json};
use inject::ActiveToolsPush;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, EntryId, ModelRef};
use cyrup_ext::caps::http::HttpCaps;
use cyrup_ext::caps::proc::ProcCaps;
use cyrup_ext::host::{
    ControlOp, CustomSpec, DialogOptions, ExecOutput, HostServices, HttpRequest, HttpResponse,
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
    /// The session's scoped-model set, pre-serialized in pi's `ScopedModel` shape
    /// (`{model, thinkingLevel?}` — `core/model-resolver.ts:63-67` @v0.83.0), backing
    /// [`HostServices::scoped_models`] (pi `ctx.scopedModels`, `core/extensions/types.ts:326`,
    /// bound on the BASE context by `getScopedModels()`, `core/extensions/runner.ts:706-709`).
    ///
    /// Mirrored for the same reason [`Self::system_prompt`] is: the authority
    /// (`AgentSession::scoped_models`, session.rs) lives behind a lock this backend does not own,
    /// and the read is SYNC. `AgentSession::set_scoped_models` — the ONLY writer, called from
    /// `main.rs` after `resolve_scoped_models_reporting` — re-seeds it on every change, so the
    /// mirror can never lag. Empty until then, which is upstream's documented "Empty when no
    /// scoping is configured".
    scoped_models: Vec<Value>,
}

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
    /// The live session's guest-facing introspection catalog (EXT-037 / EXT-038), attached
    /// post-build via [`Self::attach_session_catalog`]. `None` on the default host and on a
    /// by-value session (nothing calls `into_shared`), where [`HostServices::commands`] answers
    /// `None` so the guest binding falls back to the extension registry's own resolved commands.
    catalog: Mutex<Option<Arc<dyn SessionCatalog>>>,
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
    /// The interactive TUI's live theme seam (SEAM-T01), attached post-build via
    /// [`Self::attach_theme_access`]. `None` in RPC and in headless (print/json) — and on the
    /// default host — where all four theme methods answer pi's `noOpUIContext` values. See
    /// [`ThemeAccess`].
    theme_access: Mutex<Option<Arc<dyn ThemeAccess>>>,
    /// The interactive editor's extension-visible buffer (SEAM-T02), attached post-build via
    /// [`Self::attach_editor_mirror`]. `None` outside the interactive TUI, where
    /// [`HostServices::editor_text`] keeps pi's headless `""`. See [`EditorTextMirror`].
    editor_mirror: Mutex<Option<EditorTextMirror>>,
    /// The host-owned inter-extension event bus (Pi's single `createEventBus()`, threaded onto
    /// every `ExtensionAPI` at `extensions/loader.ts:389` @v0.83.0), attached post-build via
    /// [`Self::attach_event_bus`] — the `ExtensionHost` that owns it is built AFTER this backend,
    /// because the host is handed this backend. It is the SAME `SharedBus` every loaded WASM guest
    /// emits into, so a native's [`HostServices::emit_event`] and a guest's `bus.emit` land in one
    /// queue and are fanned out by the one drain. `None` on the default/by-value backend, where an
    /// emit is dropped (PERM-011 half B).
    event_bus: Mutex<Option<Arc<cyrup_ext::host::SharedBus>>>,
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
            catalog: Mutex::new(None),
            theme_access: Mutex::new(None),
            editor_mirror: Mutex::new(None),
            event_bus: Mutex::new(None),
        }
    }

    /// Attach the host-owned inter-extension event bus (PERM-011 half B), so a NATIVE extension's
    /// [`HostServices::emit_event`] reaches the same queue a WASM guest's `bus.emit` does.
    ///
    /// Called by the builder immediately after the `ExtensionHost` is constructed — the ordering is
    /// forced, since the host takes this backend as an argument. Every native is loaded after that
    /// point, so no emit can be issued before the bus is in place.
    pub fn attach_event_bus(&self, bus: Arc<cyrup_ext::host::SharedBus>) {
        *crate::sync::lock(&self.event_bus) = Some(bus);
    }

    /// Build with a caller-supplied fallback exec timeout (tests only; production always gets the
    /// real [`DEFAULT_EXEC_TIMEOUT`] via [`Self::new`]) — L4 review.
    #[cfg(test)]
    pub(crate) fn with_exec_timeout(provider: Arc<dyn Provider>, proc: Arc<dyn ProcOps>, cwd: PathBuf, exec_timeout: Duration) -> Self {
        Self { exec_timeout, ..Self::new(provider, proc, cwd) }
    }

    /// Push the active model + its context window (the session calls this on build + `set_model`).
    pub fn update_model(&self, model: ModelRef, context_window: u64, thinking_level: Option<String>) {
        let mut g = crate::sync::lock(&self.snapshot);
        g.model = Some(model);
        g.context_window = context_window;
        g.thinking_level = thinking_level;
    }

    /// Push session-level state (name + last-turn token occupancy) for the read views.
    pub fn update_state(&self, session_name: Option<String>, used_tokens: u64) {
        let mut g = crate::sync::lock(&self.snapshot);
        g.session_name = session_name;
        g.used_tokens = used_tokens;
    }

    /// Seed/refresh the mirrored system prompt + project-trust verdict a guest's `ctx-state` reads
    /// (EXT-005). Called by the builder once and by `AgentSession::push_active_tools` on every
    /// prompt rebuild.
    pub fn update_prompt_state(&self, system_prompt: Option<String>, project_trusted: bool) {
        let mut g = crate::sync::lock(&self.snapshot);
        if system_prompt.is_some() {
            g.system_prompt = system_prompt;
        }
        g.project_trusted = project_trusted;
    }

    /// Seed/refresh the mirrored scoped-model set a guest's `ctx.scopedModels` reads (EXT-045; pi
    /// `getScopedModels()` on the base extension context, `core/extensions/runner.ts:706-709`).
    /// Called by `AgentSession::set_scoped_models`, the one writer of the authoritative set, so the
    /// guest-visible view moves in lockstep with the `/scoped-models` command's own.
    pub fn update_scoped_models(&self, models: Vec<Value>) {
        crate::sync::lock(&self.snapshot).scoped_models = models;
    }

    /// Attach the live session's activity readback + interrupt (EXT-005). Installed by
    /// `AgentSession::into_shared` over a weak self-handle.
    pub(crate) fn attach_session_activity(&self, activity: Arc<dyn SessionActivity>) {
        *crate::sync::lock(&self.activity) = Some(activity);
    }

    /// Attach the live session's guest-facing introspection catalog (EXT-037 / EXT-038) — the
    /// source behind [`HostServices::commands`] and the extension-tool provenance half of
    /// [`HostServices::all_tools`]. Installed by `AgentSession::into_shared` over a weak
    /// self-handle, exactly like [`Self::attach_session_activity`].
    pub(crate) fn attach_session_catalog(&self, catalog: Arc<dyn SessionCatalog>) {
        *crate::sync::lock(&self.catalog) = Some(catalog);
    }

    /// Attach the interactive TUI's live theme seam (SEAM-T01) — the source behind all four of
    /// `theme`/`theme_list`/`theme_by_name`/`set_theme`. Installed by the interactive TUI ONLY,
    /// because that is the only mode pi binds them in (`createExtensionUIContext`,
    /// `interactive-mode.ts:2401-2417` @v0.84.2); leaving it unattached in RPC/print/json IS the
    /// upstream policy, not an omission. Must be re-run against every swapped-in session, exactly
    /// like the ui sinks: a replacement session brings a fresh `LiveHostServices`.
    pub fn attach_theme_access(&self, theme: Arc<dyn ThemeAccess>) {
        *crate::sync::lock(&self.theme_access) = Some(theme);
    }

    /// Attach the interactive editor's extension-visible buffer mirror (SEAM-T02) — the source
    /// behind [`HostServices::editor_text`], and the cell [`HostServices::set_editor_text`]'s
    /// replace arm writes through so a guest's own read-back is coherent. Interactive TUI only,
    /// and re-run on every session swap, for the same reasons [`Self::attach_theme_access`] is.
    pub fn attach_editor_mirror(&self, mirror: EditorTextMirror) {
        *crate::sync::lock(&self.editor_mirror) = Some(mirror);
    }

    /// Attach the command-tier control sink (the runtime owns it once the session is live).
    pub fn set_control_sink(&self, sink: ControlSink) {
        *crate::sync::lock(&self.control) = Some(sink);
    }

    /// Attach the mode's dialog renderer (the interactive TUI selector arm, or the RPC
    /// `extension_ui_request` emitter). Only interactive/rpc call this; headless (print/json) leaves it
    /// `None`, which is what keeps the ui overrides returning the deny defaults WITHOUT blocking — the
    /// absence of a sink IS the headless policy, mirroring Pi's absence of a `uiContext`.
    pub fn set_ui_sink(&self, sink: UiSink) {
        *crate::sync::lock(&self.ui_sink) = Some(sink);
    }

    /// Attach the mode's fire-and-forget effect drain (see [`UiEffectSink`]/[`Self::ui_effect_sink`]).
    /// Only interactive/rpc call this; headless (print/json) leaves it `None`.
    pub fn set_ui_effect_sink(&self, sink: UiEffectSink) {
        *crate::sync::lock(&self.ui_effect_sink) = Some(sink);
    }

    /// Attach the mode's interactive-overlay renderer (see [`OverlaySink`]/[`Self::overlay_sink`]).
    /// ONLY the interactive TUI calls this: it is the one mode that owns a terminal it can route
    /// live keystrokes from. Everything else leaves it `None`, which is what makes
    /// [`HostServices::open_overlay`] answer `false` immediately instead of blocking forever.
    pub fn set_overlay_sink(&self, sink: OverlaySink) {
        *crate::sync::lock(&self.overlay_sink) = Some(sink);
    }

    /// Send one fire-and-forget effect to the attached drain, if any (no-op — matching Pi's
    /// `noOpUIContext` — when unattached). Never blocks: an `UnboundedSender::send` never awaits.
    fn emit_ui_effect(&self, effect: UiEffect) {
        if let Some(sink) = crate::sync::lock(&self.ui_effect_sink).clone() {
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
    ///
    /// That multi-threaded runtime is NECESSARY but not SUFFICIENT. The other half is a caller-side
    /// INVARIANT THE TUI RUN LOOP UPHOLDS (TUI-092 §5b.2/§5b.4) — no caller may reach this from the
    /// interactive run-loop task — and it is load-bearing, not hygiene: `block_in_place` frees the
    /// worker THREAD for other tasks, never the calling TASK's own `poll()`, which stays parked
    /// inside this call. If the parked task is `App::run`'s loop, that loop can never come round to
    /// service `ui_rx` and answer the one-shot below — the reply this waits for is one only the
    /// blocked task could deliver. Self-deadlock, not a slow path, and it takes the keyboard with
    /// it: the same loop is the sole input-channel drain (TUI-092 §2.1). `opts.timeout_ms` is NOT
    /// the safety net — its countdown is armed INSIDE the `block_on`, so it bounds only the
    /// dialogs that carry one (`editor` never does; it passes `DialogOptions::default()`) — and no
    /// OUTER timer can bound any of them, because `tokio::time::timeout` polls its inner future
    /// FIRST and a `poll()` that never returns is never re-entered, so its `Sleep` is never polled
    /// (`cyrup-ext/src/dispatch.rs:499` wraps this same class of call in
    /// `tokio::time::timeout(self.budget, call)` and the budget still cannot fire). Enforced at the
    /// call sites in `cyrup-tui`: `app.rs`'s `AppAction::Submit` (:8142), `AppAction::FollowUp`
    /// (:8163) and `AppAction::ExtensionShortcut` (:8230) arms all `tokio::spawn` the
    /// guest-reentrant work, and `AppAction::Command` (:8224) now reaches the runtime through a
    /// channel-back instead of awaiting `App::execute_command` inline on the loop task. Same
    /// invariant, same reasons, as [`HostServices::open_overlay`]'s.
    fn ui_roundtrip(
        &self,
        kind: UiKind,
        prompt: &str,
        options: Value,
        message: String,
        placeholder: Option<String>,
        opts: &DialogOptions,
    ) -> Option<UiReply> {
        let sink = crate::sync::lock(&self.ui_sink).clone()?;
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
        *crate::sync::lock(&self.control_rx) = Some(rx);
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
        let mut g = crate::sync::lock(&self.control_rx);
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
            let mut snap = crate::sync::lock(&self.snapshot);
            snap.session_id = Some(mgr.session_id().as_str().to_string());
            snap.session_file = mgr.session_file().map(Path::to_path_buf);
        }
        *crate::sync::lock(&self.manager) = Some(manager);
    }

    /// Attach the late-bound message-injection sink (R-SA-101 / P-2). `AgentSession::into_shared` binds
    /// a sink that upgrades a weak self-handle and spawns the async inject/turn, so a background task
    /// calling [`HostServices::inject_message`] reaches THIS session's live turn loop. Idempotent:
    /// re-binding replaces the sink (a fresh session generation gets a fresh handle).
    pub fn set_inject_sink(&self, sink: InjectSink) {
        *crate::sync::lock(&self.inject_sink) = Some(sink);
    }

    /// Share the session's authoritative dynamic-tool view so a guest's `setActiveTools`/
    /// `getActiveTools` capability read+mutates the SAME state the host/CLI tool-toggle does (Pi
    /// `getActiveTools`/`setActiveTools`, agent-session.ts:2281,2283). The builder calls this once the
    /// shared `Arc<Mutex<DynamicToolState>>` exists (step 6), before any guest can be loaded.
    pub(crate) fn attach_dynamic_tools(&self, dynamic_tools: Arc<Mutex<DynamicToolState>>) {
        *crate::sync::lock(&self.dynamic_tools) = Some(dynamic_tools);
    }

    /// Drain the `(tools, system_prompt)` push a guest `setActiveTools` queued;
    /// [`crate::AgentSession::apply_pending_control`] applies it to the live agent before the next
    /// turn (the guest ran the restriction synchronously across the wasm-suspended call).
    pub fn take_pending_active_tools(&self) -> Option<ActiveToolsPush> {
        crate::sync::lock(&self.pending_active_tools).take()
    }

    /// Drain the facade events queued by guest state mutations (entry_appended/session_info_changed);
    /// [`crate::AgentSession::apply_pending_control`] fans them out on the live streams. The guest is
    /// wasm-suspended across the SYNC mutation, so — mirroring the control queue — the ASYNC fan-out
    /// runs at the next command-tier-safe drain.
    pub fn take_pending_events(&self) -> Vec<AgentSessionEvent> {
        std::mem::take(&mut *crate::sync::lock(&self.pending_events))
    }

    /// Acquire the attached manager without blocking (the guest host call runs on the session task
    /// while the manager lock is free — Pi appends synchronously). `Err` (never a panic) when the
    /// session is unattached or transiently busy, surfaced to the guest as a WIT `result` error.
    fn with_manager<R>(
        &self,
        f: impl FnOnce(&mut SessionManager) -> Result<R, String>,
    ) -> Result<R, String> {
        let manager = crate::sync::lock(&self.manager).clone().ok_or("session not attached")?;
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

    /// The WASM tier's `ui.custom` (SEAM: `custom` used to take the trait default `None`, so a guest
    /// could describe a component, get `none` back, and never learn that nothing was ever drawn).
    ///
    /// Native extensions were never affected — they reach the interactive form through
    /// [`Self::open_overlay`] with a real `Box<dyn InteractiveOverlay>` (the subagents fleet modal
    /// and the permission-system settings modal are live users). A WASM guest cannot pass a trait
    /// object across the component boundary, so it sends a [`CustomSpec`]; this turns that spec into
    /// a [`cyrup_ext::host::SpecOverlay`] — an `InteractiveOverlay` like any other — and drives it
    /// through the SAME `overlay_sink` the native route uses. One renderer, two producers.
    ///
    /// `None` (without blocking) when there is no interactive surface: headless print/json and RPC
    /// never install an overlay sink, and pi's own RPC mode answers this verb `undefined`
    /// unconditionally ("Custom UI not supported in RPC mode", `modes/rpc/rpc-mode.ts:228-231`
    /// @v0.84.2). An empty/unparseable spec is likewise declined rather than opening a blank modal
    /// the human has to dismiss.
    fn custom(&self, spec: &Value) -> Option<String> {
        let parsed = CustomSpec::from_json(spec);
        if parsed.is_empty() {
            // The WIT return is a bare `option<string>` with no error arm, so the diagnostic can
            // only go to the log — but it must go SOMEWHERE, or this is the silent-default defect
            // again one layer down.
            tracing::warn!(
                spec = %spec,
                "ui.custom: the spec has no title, lines or options — nothing to render"
            );
            return None;
        }
        let (overlay, result) = parsed.into_overlay();
        // BLOCKS until the human closes the modal, exactly as pi's `await ctx.ui.custom(...)` does;
        // `false` means no host took it (no interactive surface), which is pi's `!ctx.hasUI` branch.
        if !self.open_overlay(Box::new(overlay)) {
            return None;
        }
        // Read the cell the overlay published into. The renderer has already torn the modal down
        // (that is what resolved the `done` one-shot above) and routes it no further keystrokes, so
        // nothing can still be inside `handle_key` holding this lock — and even a poisoned lock is
        // recovered rather than panicked on ([`crate::sync::lock`]).
        crate::sync::lock(&result).take()
    }

    fn open_overlay(&self, overlay: Box<dyn InteractiveOverlay>) -> bool {
        // No renderer attached (headless print/json, RPC, or a bare embedder): report "not taken"
        // WITHOUT blocking, so the caller falls back to its own non-interactive surface. This is
        // pi's `if (!ctx.hasUI)` branch, expressed as a return value rather than a capability probe.
        let Some(sink) = crate::sync::lock(&self.overlay_sink).clone() else { return false };
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        if sink.send(OverlayRequest { overlay, done: done_tx }).is_err() {
            // The renderer's run loop is gone — degrade exactly as `ui_roundtrip` does.
            return false;
        }
        // Block THIS task until the renderer tears the modal down. `block_in_place` frees the
        // worker thread meanwhile, the same pattern [`Self::ui_roundtrip`] and the `exec` grant
        // use; both interactive entry points run on the multi-threaded runtime this requires.
        //
        // INVARIANT THE TUI RUN LOOP UPHOLDS (TUI-092 §5b.2/§5b.4) — no caller may reach this from
        // the interactive run-loop task. Load-bearing, not hygiene: `block_in_place` frees the
        // worker THREAD for other tasks, never the calling TASK's own `poll()`, which stays parked
        // inside this call. If the parked task is `App::run`'s loop, that loop can never come round
        // to service `overlay_rx` and resolve `done_tx` — the reply this waits for is one only the
        // blocked task could deliver. Self-deadlock, not a slow path, and it takes the keyboard
        // with it: the same loop is the sole input-channel drain (TUI-092 §2.1), so Ctrl+C/Ctrl+D
        // die with it. Nothing here can check the invariant (the host side sees only `&self`), so
        // it is stated here and enforced at the call sites in `cyrup-tui`: `app.rs`'s
        // `AppAction::Submit` (:8142), `AppAction::FollowUp` (:8163) and
        // `AppAction::ExtensionShortcut` (:8230) arms all `tokio::spawn` the guest-reentrant work,
        // and `AppAction::Command` (:8224) — which used to `.await` `App::execute_command` INLINE
        // on the loop task (TUI-092 culprit C: its session-lifecycle arms dispatch
        // `HostEvent::Session{Start,Shutdown,BeforeSwitch,BeforeFork}` to guest hooks, and a hook
        // that calls `ctx.ui().*` lands exactly here) — now reaches the runtime through a
        // channel-back, so that `.await` runs on a spawned task.
        //
        // A TIMEOUT CANNOT RESCUE A VIOLATION. `tokio::time::timeout` polls its inner future FIRST;
        // a `poll()` that never returns is never re-entered, so the outer `Sleep` is never polled
        // and the deadline is never consulted. `cyrup-ext/src/dispatch.rs:499` already wraps this
        // very class of call — every `HostEvent` hook, the session-lifecycle ones included — in
        // `tokio::time::timeout(self.budget, call)`, and the budget STILL cannot fire. The
        // caller-side invariant is the only thing holding this up.
        //
        // Deliberately NO timeout here either: an overlay is a modal the human is looking at, and
        // pi's `await ctx.ui.custom(...)` has no timer either (and per the paragraph above a timer
        // would buy no safety anyway). A dropped sender (renderer gone, session swapped, app quit)
        // resolves `done_rx` as `Err`, which is still a resolution — the block cannot outlive the
        // renderer.
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

    fn set_widget(
        &self,
        key: &str,
        lines: Option<&[String]>,
        placement: cyrup_ext::host::WidgetPlacement,
    ) {
        // SEAM-011 — pi's three arguments, carried under pi's own field names. `lines: null` is
        // pi's `content: undefined` (remove this key), which is why it is not flattened to `[]`.
        self.emit_ui_effect(UiEffect::SetWidget {
            widget: json!({
                "key": key,
                "lines": lines,
                "placement": placement.as_str(),
            }),
        });
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
        // SEAM-T02, the write-through half. Pi's `setEditorText` is a SYNCHRONOUS
        // `this.editor.setText(text)` (`interactive-mode.ts:2392` @v0.84.2), so upstream's very
        // next `getEditorText()` already returns `text`. Cyrup's write is fire-and-forget over the
        // effect sink and is not applied until the run loop drains it, so without this line a guest
        // that sets the buffer and immediately reads it back — the read-modify-write an editor
        // extension is written to do — would see the PREVIOUS text and write that back over its own
        // edit. Publishing here closes that window; the run loop's own per-frame publish then
        // confirms the same value.
        //
        // Only the REPLACE arm. `is_paste` is pi's `pasteToEditor`, which feeds bracketed-paste
        // markers through `this.editor.handleInput` (`:2391`) and INSERTS at a cursor this backend
        // does not know, so the only correct post-paste value is the editor's own — carried by the
        // next frame's publish. Guessing here would be worse than waiting.
        if !is_paste && let Some(mirror) = crate::sync::lock(&self.editor_mirror).clone() {
            mirror.publish(text);
        }
        self.emit_ui_effect(UiEffect::SetEditorText { text: text.to_string(), is_paste });
    }

    /// Pi `getEditorText()` (`core/extensions/types.ts:219` @v0.83.0), interactively
    /// `this.editor.getExpandedText?.() ?? this.editor.getText()` (`interactive-mode.ts:2393`
    /// @v0.84.2). SEAM-T02 — this used to take the trait default `String::new()` while its write
    /// half worked, which is what turned a missing read into data loss; see [`EditorTextMirror`]
    /// for the mechanism and why it is a shared cell rather than a `UiSink` round trip.
    fn editor_text(&self) -> String {
        crate::sync::lock(&self.editor_mirror).clone().map(|m| m.text()).unwrap_or_default()
    }

    // --- the theme family (SEAM-T01) ---
    // All four used to take their `HostServices` trait defaults (`None` / `json!([])` / `None` /
    // `Err`), in every mode, because `LiveHostServices` overrode none of them — so a loaded
    // extension asking for the theme got pi's HEADLESS answer even in the interactive TUI, where pi
    // binds all four to real state (`createExtensionUIContext`, `interactive-mode.ts:2401-2417`
    // @v0.84.2). EXT-066 had already ADDED `theme-get-json` to the WIT world so a guest could read
    // the ACTIVE theme's colours; `cyrup-ext/src/host/live.rs`'s `theme_get_json` composes it from
    // `theme()` + `theme_by_name()`, so that capability was designed, signed into the world and
    // shipped against two reads that could only ever answer `None`.
    //
    // An unattached [`ThemeAccess`] leaves every one of them on the default, which is deliberate
    // and correct: those defaults ARE pi's `noOpUIContext` values
    // (`core/extensions/runner.ts:261-263` @v0.83.0) and pi's RPC-mode values
    // (`modes/rpc/rpc-mode.ts:290-300` @v0.83.0). This is also why the switch does NOT travel the
    // [`UiEffectSink`] like its `set_*` siblings: RPC mode installs that sink, so routing it there
    // would make `setTheme` succeed over RPC, where pi hard-codes a failure.

    fn theme(&self) -> Option<String> {
        let access = crate::sync::lock(&self.theme_access).clone()?;
        access.active()
    }

    fn theme_list(&self) -> Value {
        match crate::sync::lock(&self.theme_access).clone() {
            Some(access) => access.list(),
            None => json!([]),
        }
    }

    fn theme_by_name(&self, name: &str) -> Option<Value> {
        let access = crate::sync::lock(&self.theme_access).clone()?;
        access.by_name(name)
    }

    fn set_theme(&self, name: &str) -> Result<(), String> {
        match crate::sync::lock(&self.theme_access).clone() {
            Some(access) => access.set(name),
            // Pi `noOpUIContext.setTheme: () => ({success: false, error: "UI not available"})`
            // (`core/extensions/runner.ts:263` @v0.83.0) — verbatim, rather than the trait's own
            // `"theme capability not granted"`, because reaching this backend at all means the
            // grant was given: what is missing is a UI to switch, which is upstream's exact wording
            // for that state. (Pi's RPC mode says "Theme switching not supported in RPC mode"
            // instead, `rpc-mode.ts:300`; this seam cannot tell RPC from print/json, and
            // `noOpUIContext` is the answer that is right for both of the modes cyrup routes here.)
            None => Err("UI not available".into()),
        }
    }

    fn set_tools_expanded(&self, expanded: bool) {
        self.emit_ui_effect(UiEffect::SetToolsExpanded { expanded });
    }

    // --- the working-indicator family (TUI-030) ---
    // All four used to take the `HostServices` trait default — an empty body — because there was no
    // `UiEffect` variant to push, so a loaded extension calling any of them changed NOTHING and got
    // no error and no log line. Pi's interactive mode binds every one to real TUI state
    // (`createExtensionUIContext`, `interactive-mode.ts:2377-2385` @v0.84.2); only the headless
    // modes get `noOpUIContext` (`core/extensions/runner.ts:242-245` @v0.84.2, four `() => {}`
    // bodies), which is exactly what an unattached `ui_effect_sink` reproduces here.

    fn set_working_message(&self, message: Option<&str>) {
        self.emit_ui_effect(UiEffect::SetWorkingMessage { message: message.map(str::to_string) });
    }

    fn set_working_visible(&self, visible: bool) {
        self.emit_ui_effect(UiEffect::SetWorkingVisible { visible });
    }

    fn set_working_indicator(&self, opts: Option<&Value>) {
        self.emit_ui_effect(UiEffect::SetWorkingIndicator { options: opts.cloned() });
    }

    fn set_hidden_thinking_label(&self, label: Option<&str>) {
        self.emit_ui_effect(UiEffect::SetHiddenThinkingLabel { label: label.map(str::to_string) });
    }

    fn models(&self) -> Value {
        serde_json::to_value(self.provider.models()).unwrap_or_else(|_| json!([]))
    }

    fn current_model(&self) -> Option<String> {
        crate::sync::lock(&self.snapshot)
            .model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider.as_str(), m.model.as_str()))
    }

    fn thinking_level(&self) -> Option<String> {
        crate::sync::lock(&self.snapshot).thinking_level.clone()
    }

    fn context_usage(&self) -> Value {
        let g = crate::sync::lock(&self.snapshot);
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
        crate::sync::lock(&self.snapshot).session_name.clone()
    }

    fn session_id(&self) -> Option<String> {
        // The immutable session id, cached at `attach_session`. A sync read of the snapshot (no
        // manager lock) so a background caller resolves it even while a turn holds the async lock
        // (P-2; the permission spool routes hard on this id).
        crate::sync::lock(&self.snapshot).session_id.clone()
    }

    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        // C3 (reconciliation §1 / §4 step 6): the ONE session-scoped lock BOTH companions serialize
        // their human prompts on. Every native handed this backend Arc via `set_host_services` reads
        // this SAME instance, so a permission `ask` dialog and an intercom clarify can never prompt the
        // same human simultaneously.
        Some(Arc::clone(&self.human_interaction))
    }

    /// PERM-011 half B — a native extension's `pi.events.emit`. Enqueues on the SHARED bus, so the
    /// host's next fan-out delivers it to every subscriber (guest or native) exactly as an emit
    /// from a guest's `bus.emit` import is delivered. Dropped when no bus is attached (a by-value
    /// session), which is the same tier of "no backend, no effect" as the ui-effect sink.
    fn emit_event(&self, topic: &str, payload: &Value) {
        let bus = crate::sync::lock(&self.event_bus).clone();
        if let Some(bus) = bus {
            bus.emit(topic.to_string(), payload.clone());
        }
    }

    fn session_file(&self) -> Option<PathBuf> {
        // The LIVE persisted file (deferred until the first assistant message; changes on fork), read
        // from the attached tree manager. `Ok(_)` — attached and read (the value may itself be `None`
        // if not yet persisted); `Err(_)` — unattached OR the lock is momentarily contended (a
        // background caller during an in-progress turn), in which case fall back to the cached
        // snapshot (P-2). Non-blocking `try_lock` (via `with_manager`) — never a hang, never a panic.
        match self.with_manager(|mgr| Ok(mgr.session_file().map(Path::to_path_buf))) {
            Ok(file) => file,
            Err(_) => crate::sync::lock(&self.snapshot).session_file.clone(),
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
        let sink = crate::sync::lock(&self.inject_sink)
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
            && let Some(activity) = crate::sync::lock(&self.activity).clone()
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
        let sink = crate::sync::lock(&self.control).clone();
        match sink {
            Some(f) => f(op),
            None => Err("control capability not yet wired to a runtime".into()),
        }
    }

    // ---- EXT-005 `ctx-state` reads (Pi agent-session.ts:2409-2434) ----------------------------

    fn is_idle(&self) -> bool {
        // Live, never mirrored: a handler asks precisely while a run is in flight.
        match crate::sync::lock(&self.activity).clone() {
            Some(a) => a.is_idle(),
            None => true,
        }
    }

    fn has_pending_messages(&self) -> bool {
        match crate::sync::lock(&self.activity).clone() {
            Some(a) => a.pending_message_count() > 0,
            None => false,
        }
    }

    fn is_project_trusted(&self) -> bool {
        crate::sync::lock(&self.snapshot).project_trusted
    }

    fn system_prompt(&self) -> Option<String> {
        crate::sync::lock(&self.snapshot).system_prompt.clone()
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
        // The timeout key is pi's `timeout`, not `timeoutMs`: `ExecOptions { signal?: AbortSignal;
        // timeout?: number; cwd?: string }` at `core/exec.ts:11-18` @v0.83.0, `timeout?: number` on
        // `:15` ("Timeout in milliseconds"). This read accepted ONLY `timeoutMs` — cyrup's own SDK
        // spelling (`cyrup-ext-sdk/src/descriptor.rs` `ExecOptions`) — so both halves agreed and the
        // divergence was invisible, exactly as EXT-048 describes for the sibling dialog bag. It stops
        // being invisible the moment anything else writes the bag: a hand-written guest or a ported pi
        // extension sending `{timeout: 5000}` had it silently ignored and got the fallback ceiling
        // instead of its own bound, with no error. `timeout` is canonical; `timeoutMs` is accepted for
        // the bags cyrup's SDK already writes.
        //
        // L4 review: falls back to [`DEFAULT_EXEC_TIMEOUT`] (`self.exec_timeout`) when the guest gave
        // neither key (or gave `0`) — see that constant's doc for why an unbounded `exec` here,
        // unlike Pi's own unbounded `execCommand`, has no live abort escape hatch to fall back on.
        let timeout = opts
            .get("timeout")
            .or_else(|| opts.get("timeoutMs"))
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
        crate::sync::lock(&self.pending_events).push(AgentSessionEvent::EntryAppended { entry });
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
            crate::sync::lock(&self.snapshot).session_name = resolved.clone();
            crate::sync::lock(&self.pending_events)
                .push(AgentSessionEvent::SessionInfoChanged { name: resolved });
        }
    }

    // --- session READ-only view (pi `ctx.sessionManager: ReadonlySessionManager`,
    // `core/extensions/types.ts:317` @v0.83.0, bound on the BASE extension context by
    // `get sessionManager() { runner.assertActive(); return runner.sessionManager }`,
    // `core/extensions/runner.ts:694-697` — so pi answers these truthfully in EVERY mode, tui/rpc/
    // json/print alike; there is no upstream variant that reports an empty session).
    //
    // The write half of this same interface (`append_entry`/`set_session_name`/`set_label`, just
    // above) has always been overridden, which is what made the read half's silence invisible: the
    // seam looks wired at every call site and only one direction lies. `Err` from `with_manager`
    // (unattached, or the non-blocking `try_lock` momentarily contended by an in-progress turn)
    // degrades to the trait default exactly as the sibling `session_file` read already does —
    // never a block, never a panic.

    fn entries(&self) -> Value {
        // pi `SessionManager.getEntries()` (`core/session-manager.ts:1301`): every entry except the
        // session header, in file order. Same serialization `AgentSession::entries_json` performs.
        self.with_manager(|mgr| {
            Ok(Value::Array(
                mgr.entries().iter().filter_map(|e| serde_json::to_value(e).ok()).collect(),
            ))
        })
        .unwrap_or_else(|_| json!([]))
    }

    fn branch(&self) -> Value {
        // pi `SessionManager.getBranch()` (`core/session-manager.ts:1260`), the root→leaf path from
        // the CURRENT leaf — pi's own no-argument call site shape (`core/sdk.ts:192`,
        // `core/agent-session.ts:1802`), which is `branch_path(None)` here.
        self.with_manager(|mgr| {
            Ok(Value::Array(
                mgr.branch_path(None).iter().filter_map(|e| serde_json::to_value(e).ok()).collect(),
            ))
        })
        .unwrap_or_else(|_| json!([]))
    }

    fn tree(&self) -> Value {
        // pi `SessionManager.getTree()` (`core/session-manager.ts:1306`) → `SessionTreeNode[]`.
        // Shares [`tree_node_to_json`] with `AgentSession::tree_json`, so the `labelTimestamp`
        // SEAM-060 restored on the RPC side cannot go missing on this one.
        self.with_manager(|mgr| Ok(Value::Array(mgr.tree().iter().map(tree_node_to_json).collect())))
            .unwrap_or(Value::Null)
    }

    fn scoped_models(&self) -> Value {
        // EXT-045; pi `ctx.scopedModels` (`core/extensions/types.ts:326` @v0.83.0), bound on the
        // base context as `getScopedModels()` (`core/extensions/runner.ts:706-709`) and backed by
        // `getScopedModels: () => this._scopedModels` (`core/agent-session.ts:2416`). Reads the
        // mirror `AgentSession::set_scoped_models` keeps current; `[]` until then, which is
        // upstream's documented "Empty when no scoping is configured".
        Value::Array(crate::sync::lock(&self.snapshot).scoped_models.clone())
    }

    // --- provider OAuth login callbacks (pi `OAuthLoginCallbacks`; the real wiring is
    // `onPrompt: (prompt) => callbacks.prompt({ type: "text", ...prompt })` and
    // `onSelect: (prompt) => callbacks.prompt({ type: "select", ...prompt })`,
    // `core/provider-composer.ts:245,248` @v0.83.0, against `AuthInteraction.prompt(prompt):
    // Promise<string>` — "returns the entered/selected string (`select` returns the option id).
    // Rejects on cancel/abort" — `packages/ai/src/auth/types.ts:152-161`).
    //
    // These are the SAME human-paced dialog shape `confirm`/`input`/`select` already ride, so they
    // ride the SAME `ui_sink` round trip; the two bridge-site comments in `cyrup-ext`'s
    // `host/live.rs` (`:911-914`, `:929-930`) anticipate exactly this. With NO sink attached
    // (headless print/json) they keep the trait deny defaults WITHOUT blocking — pi's
    // `noOpUIContext` — which is why the sink presence is probed before the round trip rather than
    // inferred from a `None` reply (a `None` reply also means "cancelled", and cancelled is
    // upstream's REJECT, not "no surface").

    fn oauth_prompt(
        &self,
        message: &str,
        placeholder: Option<&str>,
        allow_empty: bool,
    ) -> Result<String, String> {
        // Bound the guard to its own statement: `ui_roundtrip` takes this SAME std `Mutex`, which is
        // not reentrant, so the probe must never be alive across the call.
        let attached = crate::sync::lock(&self.ui_sink).is_some();
        if !attached {
            return Err("oauth prompt capability not granted".into());
        }
        let reply = self.ui_roundtrip(
            UiKind::Input,
            message,
            Value::Null,
            String::new(),
            placeholder.map(str::to_string),
            &DialogOptions::default(),
        );
        match reply {
            // `allow-empty: false` is the guest declaring the value mandatory (world.wit:871), so an
            // empty submission is not an answer — pi's own prompt components re-prompt rather than
            // resolve. Cyrup cannot re-prompt across the suspended guest call, so it reports the
            // step unsatisfied, which is upstream's reject-on-no-value.
            Some(UiReply::Text(Some(text))) if allow_empty || !text.trim().is_empty() => Ok(text),
            Some(UiReply::Text(Some(_))) => Err("oauth prompt cancelled: a value is required".into()),
            // Esc / timeout / a dropped renderer — pi's `prompt()` REJECTS on cancel/abort.
            _ => Err("oauth prompt cancelled".into()),
        }
    }

    fn oauth_select(&self, message: &str, options: &Value) -> Option<String> {
        // Same non-reentrancy note as `oauth_prompt`: the guard dies before `ui_roundtrip` re-locks.
        let attached = crate::sync::lock(&self.ui_sink).is_some();
        if !attached {
            return None;
        }
        // CYRUP-DELTA (`packages/ai/src/auth/types.ts:128` @v0.83.0, the `select` `AuthPrompt`
        // variant `{id, label, description?}`): the shared [`UiRequest`] carries `options` as a flat
        // array of option STRINGS and replies with the chosen STRING (the TUI's `UiKind::Select` arm
        // and the RPC `method:"select"` wire both read it that way). The OAuth selector's options are
        // `{id, label}` OBJECTS and it must return the chosen ID. Rather than widen `UiRequest` —
        // which would break both renderers — project `label` (falling back to `id`) out for display
        // and map the answer back to its `id` here, so the guest still receives exactly the id pi's
        // `callbacks.prompt({type:"select"})` returns. `description` has no carrier and is dropped.
        let rows: Vec<(String, String)> = options
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        let id = o.get("id").and_then(Value::as_str)?;
                        let label = o.get("label").and_then(Value::as_str).unwrap_or(id);
                        Some((id.to_string(), label.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let labels = Value::Array(rows.iter().map(|(_, l)| Value::String(l.clone())).collect());
        let picked = match self.ui_roundtrip(
            UiKind::Select,
            message,
            labels,
            String::new(),
            None,
            &DialogOptions::default(),
        ) {
            Some(UiReply::Text(Some(t))) => t,
            // `none` = cancel (world.wit:874), which is upstream's rejected prompt.
            _ => return None,
        };
        // Map the chosen LABEL back to its id; a guest that labelled two options identically gets
        // the first, and a renderer that echoed an id straight back still resolves.
        rows.iter()
            .find(|(_, l)| *l == picked)
            .or_else(|| rows.iter().find(|(id, _)| *id == picked))
            .map(|(id, _)| id.clone())
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

    fn system_prompt_options(&self) -> Option<Value> {
        // EXT-061. pi keeps the bag and the string in lockstep: `_rebuildSystemPrompt` assigns
        // `this._baseSystemPromptOptions` on the way to producing the prompt
        // (`core/agent-session.ts:1044-1053` @v0.83.0) and `getSystemPromptOptions` returns that
        // field verbatim (`:2436`). cyrup's analog is the SAME structure the next rebuild would
        // consume — `PromptRebuilder::base` plus the live active set — so the bag a command handler
        // reads cannot drift from the prompt `system_prompt()` returns.
        //
        // `None` until the shared dynamic-tool view is attached (default host, no live agent): the
        // WIT import then answers pi's own no-backend default `{cwd}`
        // (`core/extensions/runner.ts:287` @v0.83.0) rather than an error — see
        // `cyrup-ext/src/host/live.rs::get_system_prompt_options`.
        let dt = crate::sync::lock(&self.dynamic_tools).clone()?;
        Some(crate::sync::lock(&dt).base_prompt_options())
    }

    fn active_tools(&self) -> Option<Vec<String>> {
        // The live session's REAL active tool set (Pi `getActiveTools` = `getActiveToolNames`,
        // agent-session.ts:2281,813). `None` until the shared view is attached (default host).
        let dt = crate::sync::lock(&self.dynamic_tools).clone()?;
        Some(crate::sync::lock(&dt).active_names())
    }

    fn all_tool_names(&self) -> Option<Vec<String>> {
        // The live session's FULL registered tool set (Pi `getAllTools`, agent-session.ts:790-799) —
        // the whole enable-able `_toolRegistry`, NOT the exposed subset `active_tools` returns. This is
        // the `getAllTools` analog the permission companion's registry / unknown-tool gate checks
        // against (pi-permission-system index.ts:2218-2228). `None` until the shared dynamic-tool view
        // is attached (default host: no live agent → the companion skips the registry gate).
        let dt = crate::sync::lock(&self.dynamic_tools).clone()?;
        Some(crate::sync::lock(&dt).all().into_iter().map(|t| t.name).collect())
    }

    fn set_active_tools(&self, names: &[String]) {
        // Restrict the live agent's tool set (Pi `setActiveTools` = `setActiveToolsByName`,
        // agent-session.ts:2283,840-855). Update the authoritative dynamic-tool view SYNCHRONOUSLY —
        // Pi mutates `this.agent.state.tools` immediately, so the paired `getActiveTools` read
        // reflects it at once — and queue the rebuilt `(tools, prompt)` for the ASYNC agent push
        // `AgentSession::apply_pending_control` applies before the next turn (the guest is
        // wasm-suspended across this SYNC call, the same sync→async bridge control ops use). No-op
        // when no shared view is attached (default host: no live agent to restrict).
        let Some(dt) = crate::sync::lock(&self.dynamic_tools).clone() else { return };
        let (tools, prompt) = { crate::sync::lock(&dt).set_active(names) };
        *crate::sync::lock(&self.pending_active_tools) = Some((tools, prompt));
    }

    fn all_tools(&self) -> Option<Vec<Value>> {
        // EXT-038. pi's `getAllTools()` maps `this._toolDefinitions` — the MERGED definition
        // registry (built-ins + SDK custom + extension tools) — to
        // `{name, description, parameters, promptGuidelines, sourceInfo}`
        // (`core/agent-session.ts:906-914` @v0.83.0; type `ToolInfo`, `extensions/types.ts:1552-1554`,
        // which is `Pick<ToolDefinition, "name"|"description"|"parameters"|"promptGuidelines"> &
        // {sourceInfo}` — those five keys and NO others). cyrup's port of `_toolDefinitions` is the
        // shared `DynamicToolState`, so that is what this reads: the guest-facing `getAllTools` now
        // reports read/write/edit/bash, not the extension-only view `registry.tool_info()` gives.
        //
        // Load-bearing rather than cosmetic: this is the introspection a plan-mode / tool-restriction
        // extension reads BEFORE calling `setActiveTools`, and `set_active_tools` (just above) DOES
        // route to the live agent — so an extension-only read produced a restriction set that
        // silently omitted every built-in and then had that restriction honoured.
        //
        // `None` only when no dynamic-tool view is attached (the default host), which is what keeps
        // the `cyrup-ext` binding's registry fallback reachable.
        let dt = crate::sync::lock(&self.dynamic_tools).clone()?;
        // Snapshot the tools and RELEASE the registry lock before touching the catalog: the catalog
        // upgrades a weak `AgentSession` and takes the extension registry's lock, and nothing may
        // hold two of these at once.
        let tools = { crate::sync::lock(&dt).tools() };
        let catalog = crate::sync::lock(&self.catalog).clone();
        let ext_source_info = catalog.map(|c| c.extension_tool_source_info()).unwrap_or_default();
        Some(
            tools
                .iter()
                .map(|t| {
                    let name = t.name();
                    json!({
                        "name": name,
                        "description": t.description(),
                        "parameters": t.parameters(),
                        // EXT-007/TOOL-021 widened `Tool::prompt_guidelines` to an OWNED `Vec<&str>`
                        // precisely so a WASM guest tool's decoded guidelines are readable here.
                        "promptGuidelines": t.prompt_guidelines(),
                        "sourceInfo": ext_source_info
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| builtin_tool_source_info(name)),
                    })
                })
                .collect(),
        )
        // CYRUP-DELTA (`core/agent-session.ts:2471-2488` @v0.83.0): pi's `_toolDefinitions` is a JS
        // `Map` seeded from `_baseToolDefinitions` and then `set` per extension/SDK tool, so
        // `getAllTools()` emits built-ins in registration order followed by the extension tools, with
        // an override keeping the built-in's SLOT. cyrup's `DynamicToolState.registry` is a
        // `BTreeMap`, so the rows come out name-sorted. The dedup rule is identical (by name,
        // last registration wins the definition); only the ordering differs, and it is deterministic
        // — which the `HashMap::keys()` walk this replaces was not.
    }

    fn commands(&self) -> Option<Vec<Value>> {
        // EXT-037. pi's `getCommands()` is `[...extensionCommands, ...templates, ...skills]`, in
        // that order and with no dedup across the three (`core/agent-session.ts:2332-2354`
        // @v0.83.0), extension rows keyed on `command.invocationName` so a colliding second `deploy`
        // is handed to the guest as the `deploy:2` it can actually invoke. cyrup's port of that
        // exact concatenation is `AgentSession::slash_command_catalog`, which the catalog handle
        // delegates to — it is the only source that has the prompt templates and skills at all.
        //
        // `None` when no catalog is attached (the default host, and a by-value session that never
        // called `into_shared`); the `cyrup-ext` binding then falls back to the registry's RESOLVED
        // commands, which are extension-only but at least carry the `name:N` invocation names.
        let catalog = crate::sync::lock(&self.catalog).clone()?;
        Some(catalog.commands())
    }
}

/// The `getActiveTools` source the registered-tool wrapper diffs around every `execute` (Pi binds
/// `runtime.getActiveTools` from the session's own actions, extensions/runner.ts:329 — EXT-072 class: `:330` is the `getAllTools` wiring, and
/// `wrapRegisteredTool` calls it either side of the tool, extensions/wrapper.ts:23-25).
///
/// Deliberately the SAME read `HostServices::active_tools` performs — the authoritative
/// `DynamicToolState`, which `set_active_tools` mutates SYNCHRONOUSLY. That synchronicity is what
/// makes the diff observable: a tool that calls `setActiveTools` during its own `execute` has
/// already widened this view by the time the wrapper takes its "after" snapshot.
impl cyrup_ext::ActiveToolNames for LiveHostServices {
    fn active_tool_names(&self) -> Option<Vec<String>> {
        let dt = crate::sync::lock(&self.dynamic_tools).clone()?;
        Some(crate::sync::lock(&dt).active_names())
    }
}

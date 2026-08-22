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
    /// The Pi `ExtensionUIDialogOptions` bag — `{signal?: AbortSignal; timeout?: number}`
    /// (`extensions/types.ts:95-101` @v0.83.0, `timeout?: number` at `:100`, documented "Timeout in
    /// milliseconds. Dialog auto-dismisses with live countdown display"). EXT-048: the wire key is
    /// `timeout`, NOT `timeoutMs` — this comment used to assert the opposite and cite `types.ts:89`,
    /// which is a `keybindings.ts` re-export, not this interface. `DialogOptions` accepts `timeoutMs`
    /// as a serde alias for the bags cyrup's own SDK already writes. `signalId` is the cyrup
    /// component-boundary stand-in for `signal` (an `AbortSignal` is not a component value).
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
    /// Pi `setWidget(key, content, options?)`, types.ts:164-173 @v0.83.0; RPC wire
    /// `method:"setWidget"` (rpc-mode.ts:193-206).
    ///
    /// SEAM-011/EXT-047: the WIT no longer collapses pi's three arguments — `set-widget` carries
    /// `key`, `lines` and `placement` separately and the host receives them as
    /// [`cyrup_ext::host::WidgetPlacement`] + `Option<&[String]>`. This variant is a FRONT-END
    /// CHANNEL carrier, not the wire, so it keeps one JSON object; the object's keys are pi's own
    /// (`key`, `lines`, `placement`) and every consumer — the TUI's `ExtensionWidget::from_json` and
    /// `cyrup-modes`' `extension_ui_effect_json`, which projects them onto pi's
    /// `widgetKey`/`widgetLines`/`widgetPlacement` — reads them by those names.
    ///
    /// `lines: null` is pi's `content: undefined` and REMOVES the key's widget
    /// (`interactive-mode.ts:1935-1938`); it is never merely an empty list.
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

    // --- the working-indicator family (TUI-030) ---
    // These four had NO carrier at all: `LiveHostServices` left all four `HostServices` methods on
    // their trait defaults because there was no variant to push, so `set_working_message`,
    // `set_working_visible`, `set_working_indicator` and `set_hidden_thinking_label` were silent
    // no-ops in every mode — for native extensions and WASM guests alike, since
    // `cyrup-ext/src/host/live.rs` forwards the guest imports to these same trait methods.
    //
    // Pi's RPC mode forwards NONE of the four (`rpc-mode.ts:179-193` @v0.84.2, four empty bodies
    // whose comments read "not supported in RPC mode - requires TUI loader access" ×3 and
    // "requires TUI message rendering access"), so `cyrup_modes::rpc::extension_ui_effect_json`
    // returns `None` for all four — the same treatment `SetHeader`/`SetFooter`/`SetToolsExpanded`
    // already get, and for the same upstream reason. They travel this channel because the
    // INTERACTIVE TUI is a real consumer.
    /// Pi `setWorkingMessage(message?)`, `extensions/types.ts:151` @v0.83.0; the interactive handler
    /// is `interactive-mode.ts:2377-2382` @v0.84.2. `None` is upstream's no-argument call — restore
    /// `defaultWorkingMessage` (`"Working..."`, `:434`).
    SetWorkingMessage { message: Option<String> },
    /// Pi `setWorkingVisible(visible)`, `extensions/types.ts:154` @v0.83.0; the interactive handler
    /// is `interactive-mode.ts:2091-2108` @v0.84.2. Independent of the message, which is exactly what
    /// cyrup's collapsed `working-start(label)`/`working-stop()` pair could not express.
    SetWorkingVisible { visible: bool },
    /// Pi `setWorkingIndicator(options?)`, `extensions/types.ts:164` @v0.83.0
    /// (`WorkingIndicatorOptions {frames?, intervalMs?}` at `:116-121`); the interactive handler is
    /// `interactive-mode.ts:2110-2116` @v0.84.2. `None` restores the default animated Braille spinner.
    SetWorkingIndicator { options: Option<Value> },
    /// Pi `setHiddenThinkingLabel(label?)`, `extensions/types.ts:167` @v0.83.0; the interactive
    /// handler is `interactive-mode.ts:2118-2129` @v0.84.2. `None` restores `defaultHiddenThinkingLabel`
    /// (`"Thinking..."`, `:435`).
    SetHiddenThinkingLabel { label: Option<String> },
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

/// The live session's activity readback + interrupt, backing the `ctx-state`/`control` imports that
/// only the running session can answer (EXT-005). Pi binds these straight to the session object:
/// `isIdle: () => this.isIdle`, `hasPendingMessages: () => this.pendingMessageCount > 0` and
/// `abort: () => { void this.abort() }` (agent-session.ts:2409-2419).
///
/// A separate trait rather than more snapshot fields because these are LIVE — a mirrored `is_idle`
/// would be stale exactly when it matters (mid-run, which is when a handler asks). Attached by
/// `AgentSession::into_shared` over a weak self-handle, so it never keeps the session alive.
pub(crate) trait SessionActivity: Send + Sync {
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

/// The live session's guest-facing INTROSPECTION catalog — the two listings only the running
/// session can compose (EXT-037 / EXT-038). Pi binds both straight to the session object in
/// `_bindExtensionCore`: `getAllTools: () => this.getAllTools()` (agent-session.ts:2394) and the
/// `getCommands` closure (`:2332-2354`, bound at `:2397`) @v0.83.0.
///
/// A separate trait for the same reason [`SessionActivity`] is one: these are LIVE reads over state
/// this backend does not own, and a mirrored copy would be stale exactly when a handler asks —
/// `getCommands()` must see a command an extension registered a moment ago, and `getAllTools()`
/// must see a tool `refreshTools` just merged. Attached by `AgentSession::into_shared` over a weak
/// self-handle, so it never keeps the session alive.
pub(crate) trait SessionCatalog: Send + Sync {
    /// pi `getCommands(): SlashCommandInfo[]` — `[...extensionCommands, ...templates, ...skills]`,
    /// extension rows keyed on `command.invocationName` (`core/agent-session.ts:2332-2354`
    /// @v0.83.0; type `SlashCommandInfo`, `core/slash-commands.ts:6-11`). That is exactly
    /// `AgentSession::slash_command_catalog`, which is the ONLY source carrying prompt templates
    /// and skills — the registry fallback in `cyrup-ext` has extension commands and nothing else.
    fn commands(&self) -> Vec<Value>;

    /// The `SourceInfo` (`core/source-info.ts:6-12` @v0.83.0) pi stamps on each `_toolDefinitions`
    /// entry, for the EXTENSION-contributed tools only, keyed by tool name.
    ///
    /// pi tags the registry three ways while rebuilding it (`_refreshToolRegistry`,
    /// `agent-session.ts:2455-2488`): a built-in gets `createSyntheticSourceInfo("<builtin:${name}>",
    /// {source: "builtin"})`, an SDK custom tool `("<sdk:${name}>", {source: "sdk"})`, and a
    /// registered extension tool carries the runner's real `tool.sourceInfo`. Only the last is
    /// recoverable here, so a name absent from this map falls back to the builtin synthetic form
    /// (see `builtin_tool_source_info` below).
    fn extension_tool_source_info(&self) -> std::collections::HashMap<String, Value>;
}

/// The interactive TUI's live THEME seam — the source behind all four of
/// [`HostServices::theme`], [`HostServices::theme_list`], [`HostServices::theme_by_name`] and
/// [`HostServices::set_theme`] (SEAM-T01).
///
/// One handle for all four because pi gates all four the same way: they are bound only inside
/// `createExtensionUIContext`, which ONLY the interactive mode builds
/// (`modes/interactive/interactive-mode.ts:2404-2415` @v0.84.2 — `getAllThemes: () =>
/// getAvailableThemesWithPaths()`, `getTheme: (name) => getThemeByName(name)`, the `get theme()`
/// accessor at `:2401-2403`, and the `setTheme` closure at `:2406-2417`). Every other mode gets
/// `noOpUIContext`, whose theme members are `getAllThemes: () => []`, `getTheme: () => undefined`
/// and `setTheme: () => ({success: false, error: "UI not available"})`
/// (`core/extensions/runner.ts:261-263` @v0.83.0); pi's RPC mode hard-codes the same three answers
/// (`modes/rpc/rpc-mode.ts:290-300` @v0.83.0, its `setTheme` erroring "Theme switching not
/// supported in RPC mode"). So an UNATTACHED handle here reproduces upstream exactly: the trait
/// defaults `None` / `json!([])` / `None` are already pi's headless answers, and
/// [`LiveHostServices::set_theme`] returns pi's own `"UI not available"` string.
///
/// A trait rather than more [`LiveSnapshot`] fields for the reason the crate-internal
/// `SessionActivity` is one: the
/// active theme is LIVE (an extension asking mid-session must see a `/settings → theme` switch the
/// user made a keystroke ago), and `set` is a real ACTION whose success/failure pi returns
/// synchronously. Attached by the interactive TUI only, over handles that do not keep the app
/// alive.
pub trait ThemeAccess: Send + Sync {
    /// The ACTIVE theme's name — pi's `get theme() { return theme }`
    /// (`interactive-mode.ts:2401-2403`), reduced to the name because cyrup's WIT `theme-get`
    /// returns `option<string>`; the colours travel through [`Self::by_name`], which is how
    /// `live.rs`'s `theme-get-json` (EXT-066) composes pi's whole `Theme` value.
    fn active(&self) -> Option<String>;

    /// pi `getAllThemes(): {name, path}[]` (`core/extensions/types.ts:269` @v0.83.0), implemented
    /// upstream by `getAvailableThemesWithPaths()`
    /// (`modes/interactive/theme/theme.ts:493-520` @v0.83.0): built-ins, then custom themes, then
    /// registered ones, deduped first-wins by name and sorted by name.
    fn list(&self) -> Value;

    /// pi `getTheme(name): Theme | undefined` (`core/extensions/types.ts:272` @v0.83.0) —
    /// `getThemeByName` (`theme.ts:671-677`), which loads WITHOUT switching and swallows a load
    /// failure into `undefined`.
    fn by_name(&self, name: &str) -> Option<Value>;

    /// pi `setTheme(name): {success, error?}` (`core/extensions/types.ts:275` @v0.83.0). `Err` is
    /// upstream's `{success: false, error}`, whose message for an unknown name is
    /// `Theme not found: {name}` (`theme.ts:622`, thrown by `loadThemeJson` and caught into the
    /// result by `setTheme`, `:891-913`).
    fn set(&self, name: &str) -> Result<(), String>;
}

/// The extension-visible mirror of the interactive editor's buffer, backing
/// [`HostServices::editor_text`] (pi `getEditorText()`, `core/extensions/types.ts:219` @v0.83.0;
/// bound interactively as `getEditorText: () => this.editor.getExpandedText?.() ?? this.editor.getText()`,
/// `modes/interactive/interactive-mode.ts:2393` @v0.84.2) — SEAM-T02.
///
/// **Why a mirror and not a round trip.** The obvious alternative was a request/reply through a
/// [`UiSink`]-shaped channel, so the run loop could read `state.editor` itself. That is the wrong
/// mechanism: pi's `getEditorText()` is a plain synchronous property read that never yields to the
/// event loop, and — unlike `confirm`/`input`/`select`/`editor`, which take
/// `ExtensionUIDialogOptions {signal?, timeout?}` (`core/extensions/types.ts:95-101`) precisely
/// BECAUSE they block — it takes no options at all, so a round trip here would have no timeout to
/// bound it. [`LiveHostServices::ui_roundtrip`] parks the guest in
/// `block_in_place` + `block_on`; doing that for a getter would hand an extension a way to wedge
/// itself forever any time the run loop is not sitting at its `select!` (mid-`execute_command`,
/// mid-dialog, mid-overlay). A shared cell keeps the read synchronous, non-blocking and
/// unwedgeable, exactly as upstream's is.
///
/// **Who writes it.** Two writers, and both are needed:
/// 1. the interactive app, once per frame from [`Self::publish`] — the buffer as the user can
///    actually see it, and the reason the value tracks typing at all; and
/// 2. [`HostServices::set_editor_text`]'s REPLACE arm, which publishes the text it is about to
///    hand the run loop. Without that write the read half would still be broken for the one
///    sequence that matters most: cyrup's `setEditorText` is fire-and-forget over the
///    [`UiEffectSink`] while pi's is a synchronous `this.editor.setText(text)`, so a guest that
///    sets the buffer and immediately reads it back to modify it would see the PREVIOUS text and
///    write that back — losing its own edit. Pi cannot observe that window, so neither may cyrup.
///
/// The paste arm (`is_paste = true`, pi `pasteToEditor` → `this.editor.handleInput("\x1b[200~…")`,
/// `interactive-mode.ts:2391`) deliberately does NOT write here: an insert lands at a cursor the
/// host does not know, so the only correct value is the one the editor computes, and the next
/// frame's [`Self::publish`] is what carries it.
///
/// Unattached (`None` on [`LiveHostServices`]) in every non-interactive mode, where
/// [`HostServices::editor_text`] keeps the trait default `String::new()` — pi's own answer in
/// exactly those modes (`noOpUIContext.getEditorText: () => ""`, `core/extensions/runner.ts:253`;
/// `rpc-mode.ts:248-252`, "Synchronous method can't wait for RPC response").
#[derive(Clone, Debug, Default)]
pub struct EditorTextMirror(Arc<Mutex<String>>);

impl EditorTextMirror {
    /// A fresh, empty mirror (the editor boots empty).
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish the live buffer. The interactive app calls this once per frame with
    /// `InputEditor::expanded_text()` — pi's `getExpandedText?.() ?? getText()`, i.e. with
    /// `[paste #N …]` markers substituted back to their content, which is what upstream hands the
    /// extension.
    pub fn publish(&self, text: impl Into<String>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = text.into();
    }

    /// The current extension-visible buffer text.
    pub fn text(&self) -> String {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// pi's synthetic `SourceInfo` for a tool the extension registry does not own — the value
/// `createSyntheticSourceInfo("<builtin:NAME>", { source: "builtin" })` produces
/// (`core/agent-session.ts:2478`, defaults from `core/source-info.ts:24-40` @v0.83.0: scope
/// `"temporary"`, origin `"top-level"`, no `baseDir`).
///
/// CYRUP-DELTA (`core/agent-session.ts:2468` @v0.83.0): pi distinguishes an SDK-supplied custom
/// tool with a THIRD tag, `("<sdk:${name}>", {source: "sdk"})`. cyrup's dynamic-tool registry (the
/// port of `_toolDefinitions`, [`crate::tools::DynamicToolState`]) folds the caller's `custom_tools`
/// into the same by-name map as the built-ins at build time (`builder.rs`, "the SDK-supplied custom
/// tools go through the same registered-tool wrapper") and keeps no provenance column, so an SDK
/// tool is indistinguishable from a built-in at this seam and reports as `builtin`.
fn builtin_tool_source_info(name: &str) -> Value {
    json!({
        "path": format!("<builtin:{name}>"),
        "source": "builtin",
        "scope": "temporary",
        "origin": "top-level",
    })
}

/// One `SessionTreeNode` (pi `core/session-manager.ts:159-166`) as `{entry, children, label?,
/// labelTimestamp?}` — the shape pi's `getTree()` hands out and the wire contract names
/// (`modes/rpc/rpc-types.ts:202-208`).
///
/// Lives here rather than nested inside [`crate::AgentSession::tree_json`] because BOTH the RPC
/// `get_tree` reply and the extension seam's [`HostServices::tree`] must emit the identical shape;
/// two copies is exactly how SEAM-060's dropped `labelTimestamp` survived on one side after being
/// fixed on the other.
pub(crate) fn tree_node_to_json(node: &cyrup_session::manager::TreeNode) -> Value {
    let mut obj = serde_json::Map::new();
    if let Ok(entry) = serde_json::to_value(&node.entry) {
        obj.insert("entry".to_string(), entry);
    }
    obj.insert(
        "children".to_string(),
        Value::Array(node.children.iter().map(tree_node_to_json).collect()),
    );
    if let Some(label) = &node.label {
        obj.insert("label".to_string(), Value::String(label.clone()));
    }
    if let Some(ts) = &node.label_timestamp {
        obj.insert("labelTimestamp".to_string(), Value::String(ts.clone()));
    }
    Value::Object(obj)
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
        *Self::lock(&self.event_bus) = Some(bus);
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

    /// Seed/refresh the mirrored scoped-model set a guest's `ctx.scopedModels` reads (EXT-045; pi
    /// `getScopedModels()` on the base extension context, `core/extensions/runner.ts:706-709`).
    /// Called by `AgentSession::set_scoped_models`, the one writer of the authoritative set, so the
    /// guest-visible view moves in lockstep with the `/scoped-models` command's own.
    pub fn update_scoped_models(&self, models: Vec<Value>) {
        Self::lock(&self.snapshot).scoped_models = models;
    }

    /// Attach the live session's activity readback + interrupt (EXT-005). Installed by
    /// `AgentSession::into_shared` over a weak self-handle.
    pub(crate) fn attach_session_activity(&self, activity: Arc<dyn SessionActivity>) {
        *Self::lock(&self.activity) = Some(activity);
    }

    /// Attach the live session's guest-facing introspection catalog (EXT-037 / EXT-038) — the
    /// source behind [`HostServices::commands`] and the extension-tool provenance half of
    /// [`HostServices::all_tools`]. Installed by `AgentSession::into_shared` over a weak
    /// self-handle, exactly like [`Self::attach_session_activity`].
    pub(crate) fn attach_session_catalog(&self, catalog: Arc<dyn SessionCatalog>) {
        *Self::lock(&self.catalog) = Some(catalog);
    }

    /// Attach the interactive TUI's live theme seam (SEAM-T01) — the source behind all four of
    /// `theme`/`theme_list`/`theme_by_name`/`set_theme`. Installed by the interactive TUI ONLY,
    /// because that is the only mode pi binds them in (`createExtensionUIContext`,
    /// `interactive-mode.ts:2401-2417` @v0.84.2); leaving it unattached in RPC/print/json IS the
    /// upstream policy, not an omission. Must be re-run against every swapped-in session, exactly
    /// like the ui sinks: a replacement session brings a fresh `LiveHostServices`.
    pub fn attach_theme_access(&self, theme: Arc<dyn ThemeAccess>) {
        *Self::lock(&self.theme_access) = Some(theme);
    }

    /// Attach the interactive editor's extension-visible buffer mirror (SEAM-T02) — the source
    /// behind [`HostServices::editor_text`], and the cell [`HostServices::set_editor_text`]'s
    /// replace arm writes through so a guest's own read-back is coherent. Interactive TUI only,
    /// and re-run on every session swap, for the same reasons [`Self::attach_theme_access`] is.
    pub fn attach_editor_mirror(&self, mirror: EditorTextMirror) {
        *Self::lock(&self.editor_mirror) = Some(mirror);
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
        // recovered rather than panicked on ([`Self::lock`]).
        Self::lock(&result).take()
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
        if !is_paste && let Some(mirror) = Self::lock(&self.editor_mirror).clone() {
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
        Self::lock(&self.editor_mirror).clone().map(|m| m.text()).unwrap_or_default()
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
        let access = Self::lock(&self.theme_access).clone()?;
        access.active()
    }

    fn theme_list(&self) -> Value {
        match Self::lock(&self.theme_access).clone() {
            Some(access) => access.list(),
            None => json!([]),
        }
    }

    fn theme_by_name(&self, name: &str) -> Option<Value> {
        let access = Self::lock(&self.theme_access).clone()?;
        access.by_name(name)
    }

    fn set_theme(&self, name: &str) -> Result<(), String> {
        match Self::lock(&self.theme_access).clone() {
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

    /// PERM-011 half B — a native extension's `pi.events.emit`. Enqueues on the SHARED bus, so the
    /// host's next fan-out delivers it to every subscriber (guest or native) exactly as an emit
    /// from a guest's `bus.emit` import is delivered. Dropped when no bus is attached (a by-value
    /// session), which is the same tier of "no backend, no effect" as the ui-effect sink.
    fn emit_event(&self, topic: &str, payload: &Value) {
        let bus = Self::lock(&self.event_bus).clone();
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
        Value::Array(Self::lock(&self.snapshot).scoped_models.clone())
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
        let attached = Self::lock(&self.ui_sink).is_some();
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
        let attached = Self::lock(&self.ui_sink).is_some();
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
        let dt = Self::lock(&self.dynamic_tools).clone()?;
        Some(Self::lock(&dt).base_prompt_options())
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
        let dt = Self::lock(&self.dynamic_tools).clone()?;
        // Snapshot the tools and RELEASE the registry lock before touching the catalog: the catalog
        // upgrades a weak `AgentSession` and takes the extension registry's lock, and nothing may
        // hold two of these at once.
        let tools = { Self::lock(&dt).tools() };
        let catalog = Self::lock(&self.catalog).clone();
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
        let catalog = Self::lock(&self.catalog).clone()?;
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

        // 5) a timeout ⇒ the host SIGTERMs the group, then (since `sleep` obeys SIGTERM and dies
        //    well within the 5s grace period, no SIGKILL escalation needed here) reports
        //    `killed=true` (Pi `killProcess` sets `killed`, exec.ts:52-63). Asserted under BOTH
        //    spellings: pi's real key is `timeout` (`ExecOptions.timeout?: number`, `core/exec.ts:15`
        //    @v0.83.0) — the host used to accept ONLY cyrup's SDK spelling `timeoutMs`, so a bag
        //    written by anything else was silently ignored and fell through to the 120s ceiling.
        for key in ["timeout", "timeoutMs"] {
            let opts =
                Value::Object(serde_json::Map::from_iter([(key.to_string(), json!(100))]));
            let out = svc
                .exec("sleep", &["30".to_string()], &opts, CancelToken::new())
                .expect("sleep runs then is killed on timeout");
            assert!(out.killed, "a timed-out exec is `killed` under the `{key}` key");
        }

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

    // ---------------------------------------------------- EXT-037 / EXT-038: guest introspection --

    /// A tool double carrying the two fields pi's `ToolInfo` reads off the definition and cyrup's
    /// internal [`crate::tools::ToolInfo`] does not: `description` + `promptGuidelines`.
    struct CatalogTool {
        name: &'static str,
        params: Value,
        guidelines: Vec<&'static str>,
    }

    impl CatalogTool {
        fn new(name: &'static str, guidelines: Vec<&'static str>) -> Self {
            Self { name, params: json!({"type": "object", "properties": {}}), guidelines }
        }
    }

    #[async_trait::async_trait]
    impl Tool for CatalogTool {
        fn name(&self) -> &str {
            self.name
        }
        fn parameters(&self) -> &Value {
            &self.params
        }
        fn description(&self) -> &str {
            "described"
        }
        fn prompt_guidelines(&self) -> Vec<&str> {
            self.guidelines.clone()
        }
        async fn execute(
            &self,
            _call_id: cyrup_core::ToolCallId,
            _args: Value,
            _cancel: CancelToken,
            _on_update: cyrup_core::ToolUpdateSink,
        ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
            Ok(cyrup_core::ToolResult::default())
        }
    }

    /// A [`SessionCatalog`] double standing in for the live `AgentSession` (which a unit test here
    /// cannot build): pi's three-source command concatenation, plus the extension registry's
    /// per-tool `sourceInfo`.
    struct FakeCatalog;

    impl SessionCatalog for FakeCatalog {
        fn commands(&self) -> Vec<Value> {
            vec![
                json!({"name": "deploy", "description": "first", "source": "extension"}),
                json!({"name": "deploy:2", "description": "second", "source": "extension"}),
                json!({"name": "review", "description": "a template", "source": "prompt"}),
                json!({"name": "skill:pdf", "description": "a skill", "source": "skill"}),
            ]
        }

        fn extension_tool_source_info(&self) -> std::collections::HashMap<String, Value> {
            std::collections::HashMap::from([(
                "ext_tool".to_string(),
                json!({"path": "demo-ext", "source": "demo-ext", "scope": "temporary", "origin": "top-level"}),
            )])
        }
    }

    fn dynamic_tools_with(tools: Vec<Arc<dyn Tool>>) -> Arc<Mutex<DynamicToolState>> {
        let contributions = tools
            .iter()
            .map(|t| (t.name().to_string(), crate::builder::tool_contribution(t)))
            .collect();
        let rebuilder = crate::tools::PromptRebuilder::new(
            cyrup_session::prompt::PromptInputs::default(),
            contributions,
        );
        Arc::new(Mutex::new(DynamicToolState::new(tools.clone(), tools, rebuilder)))
    }

    /// EXT-061 — `system_prompt_options()` is the BAG behind `system_prompt()`, in pi's
    /// `BuildSystemPromptOptions` shape (`core/system-prompt.ts:8-25` @v0.83.0), sourced from the
    /// SAME `PromptRebuilder` the next prompt rebuild consumes (pi's `_baseSystemPromptOptions`,
    /// `core/agent-session.ts:1044-1053`, handed back at `:2436`).
    ///
    /// COVERAGE, NOT A REGRESSION PROOF (rule 8): the trait method is new this pass, so no form of
    /// this test could have failed against the previous HEAD. What it pins is the two properties a
    /// later edit can quietly break — that the unattached backend answers `None` (which is what
    /// routes the WIT import to pi's `{cwd}` default instead of a fabricated bag), and that the
    /// attached one reports the LIVE active set rather than the cleared `selected_tools` the
    /// rebuild base carries.
    #[test]
    fn system_prompt_options_reports_the_live_bag_behind_the_system_prompt() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // Unattached ⇒ `None`. The import layer, not this backend, supplies pi's `{cwd}` default.
        assert!(svc.system_prompt_options().is_none(), "no dynamic-tool view attached ⇒ no live bag");

        let read: Arc<dyn Tool> = Arc::new(CatalogTool::new("read", vec!["read: prefer read"]));
        let bash: Arc<dyn Tool> = Arc::new(CatalogTool::new("bash", vec![]));
        svc.attach_dynamic_tools(dynamic_tools_with(vec![read, bash]));

        let bag = svc.system_prompt_options().expect("a live dynamic-tool view answers");
        assert_eq!(
            bag["selectedTools"],
            json!(["read", "bash"]),
            "pi's bag carries `selectedTools: validToolNames` — the ACTIVE set, not the rebuild \
             base's cleared field: {bag}"
        );
        assert!(bag.get("cwd").is_some(), "`cwd` is the one REQUIRED key of pi's bag: {bag}");
        assert_eq!(
            bag["promptGuidelines"],
            json!(["read: prefer read"]),
            "each active tool's guidelines, in active order (agent-session.ts:1031-1034): {bag}"
        );
        // pi omits `customPrompt`/`appendSystemPrompt` when unset rather than emitting null.
        assert!(bag.get("customPrompt").is_none(), "an unset optional is OMITTED, not null: {bag}");
        assert!(bag.get("appendSystemPrompt").is_none(), "an unset optional is OMITTED, not null: {bag}");
    }

    /// EXT-038 — `all_tools()` must report the WHOLE merged registry (built-ins included) in pi's
    /// `ToolInfo` shape, not the extension-only view `registry.tool_info()` gives. Guards the
    /// functional half: a plan-mode extension reads this before calling `setActiveTools`, and the
    /// write IS honoured, so an extension-only read silently strips read/write/edit/bash.
    #[test]
    fn all_tools_reports_the_whole_merged_registry_in_pis_toolinfo_shape() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // Unattached: `None`, which is what keeps the cyrup-ext registry fallback reachable.
        assert!(svc.all_tools().is_none(), "no dynamic-tool view attached ⇒ no live answer");

        let builtin: Arc<dyn Tool> = Arc::new(CatalogTool::new("read", vec!["read: prefer read"]));
        let ext: Arc<dyn Tool> = Arc::new(CatalogTool::new("ext_tool", vec![]));
        svc.attach_dynamic_tools(dynamic_tools_with(vec![builtin, ext]));
        svc.attach_session_catalog(Arc::new(FakeCatalog));

        let rows = svc.all_tools().expect("a live dynamic-tool view answers");
        let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
        assert!(names.contains(&"read"), "the BUILT-IN must appear — the whole point of EXT-038: {names:?}");
        assert!(names.contains(&"ext_tool"), "the extension tool must still appear: {names:?}");

        let read = rows.iter().find(|r| r["name"] == json!("read")).expect("read row");
        // pi's `ToolInfo` is EXACTLY these five keys (`extensions/types.ts:1552-1554` @v0.83.0) —
        // no `source` discriminator (EXT-060).
        let keys: Vec<&str> = read.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["description", "name", "parameters", "promptGuidelines", "sourceInfo"],
            "pi's ToolInfo keys and no others"
        );
        assert_eq!(read["description"], json!("described"));
        assert_eq!(read["promptGuidelines"], json!(["read: prefer read"]), "guidelines must survive");
        assert_eq!(
            read["sourceInfo"],
            json!({"path": "<builtin:read>", "source": "builtin", "scope": "temporary", "origin": "top-level"}),
            "a tool the extension registry does not own gets pi's synthetic builtin SourceInfo"
        );

        let ext_row = rows.iter().find(|r| r["name"] == json!("ext_tool")).expect("ext row");
        assert_eq!(
            ext_row["sourceInfo"]["source"],
            json!("demo-ext"),
            "an extension-contributed tool keeps the REGISTRY's sourceInfo, not the builtin synthetic"
        );
    }

    /// EXT-038 — with no catalog attached the merged set is still reported (the built-ins are what
    /// matter); only the extension provenance degrades to the synthetic form.
    #[test]
    fn all_tools_without_a_catalog_still_reports_builtins() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        let builtin: Arc<dyn Tool> = Arc::new(CatalogTool::new("bash", vec![]));
        svc.attach_dynamic_tools(dynamic_tools_with(vec![builtin]));

        let rows = svc.all_tools().expect("a live dynamic-tool view answers");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], json!("bash"));
        assert_eq!(rows[0]["sourceInfo"]["source"], json!("builtin"));
    }

    // ------------------------------------------------- session read-only view (ctx.sessionManager) --

    /// The READ half of the `session` interface must answer from the LIVE tree.
    ///
    /// pi binds the real `ReadonlySessionManager` onto the BASE extension context
    /// (`get sessionManager() { runner.assertActive(); return runner.sessionManager }`,
    /// `core/extensions/runner.ts:694-697`, typed at `core/extensions/types.ts:317`), so
    /// `getEntries()`/`getBranch()`/`getTree()` are truthful in every mode upstream. cyrup's ONLY
    /// production backend overrode none of them, so every guest read `[]`/`[]`/`null` forever —
    /// indistinguishable from a genuinely fresh session, with no error and no log line, exactly the
    /// shape the EXT-005 ctx-state postmortem describes.
    ///
    /// RED before the fix on all three attached assertions (they returned the trait defaults
    /// `json!([])`/`json!([])`/`Value::Null` regardless of the attached manager); the UNATTACHED
    /// assertions pass either way and are here to pin that the honest default-host answer survives.
    #[test]
    fn session_read_view_answers_from_the_live_tree() {
        use cyrup_session::manager::NewSessionOpts;

        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // No manager attached (the default host / a by-value session): the trait defaults ARE the
        // honest answer — there is no session to report on.
        assert_eq!(svc.entries(), json!([]), "unattached ⇒ pi's empty read");
        assert_eq!(svc.branch(), json!([]), "unattached ⇒ pi's empty read");
        assert_eq!(svc.tree(), Value::Null, "unattached ⇒ the trait's null tree");

        let mut mgr = SessionManager::in_memory(&std::env::temp_dir(), NewSessionOpts::default())
            .expect("an in-memory session tree");
        let root = mgr.append_custom_entry("note", Some(json!({"n": 1}))).expect("append root");
        let leaf = mgr.append_custom_entry("note", Some(json!({"n": 2}))).expect("append leaf");
        let label = mgr.append_label(&root, Some("checkpoint")).expect("label the root");
        let (root, leaf, label) = (root.to_string(), leaf.to_string(), label.to_string());
        svc.attach_session(Arc::new(AsyncMutex::new(mgr)));

        // `entries` — pi `SessionManager.getEntries()`: every entry except the header. The label is
        // itself an appended entry, so three rows, and the two notes are among them BY ID.
        let entries = svc.entries();
        let ids: Vec<&str> =
            entries.as_array().expect("an array").iter().filter_map(|e| e["id"].as_str()).collect();
        assert!(ids.contains(&root.as_str()), "the live tree's entries reached the guest: {ids:?}");
        assert!(ids.contains(&leaf.as_str()), "the live tree's entries reached the guest: {ids:?}");

        // `branch` — pi `SessionManager.getBranch()`: walk parent-ward from the CURRENT leaf, then
        // reverse. Its doc is explicit that the walk "Includes all entry types (messages,
        // compaction, model changes, etc.)", and `appendLabelChange` builds its `LabelEntry` with
        // `parentId: this.leafId` and then `_appendEntry`s it — so labelling APPENDS to the path
        // rather than annotating off it, and the label entry is the branch head here.
        // `SessionManager::append_label` is the same mechanism (`push_entry` of a
        // `KnownEntry::Label`), so the path is asserted exactly rather than by containment.
        let branch = svc.branch();
        let branch_ids: Vec<&str> =
            branch.as_array().expect("an array").iter().filter_map(|e| e["id"].as_str()).collect();
        assert_eq!(
            branch_ids,
            vec![root.as_str(), leaf.as_str(), label.as_str()],
            "the branch is the whole root→leaf path, in order: {branch_ids:?}"
        );

        // `tree` — pi `SessionManager.getTree()` → `SessionTreeNode[]`, nested, carrying `label`
        // AND SEAM-060's `labelTimestamp` because it shares `tree_node_to_json` with the RPC
        // `get_tree` reply rather than re-deriving the node shape.
        let tree = svc.tree();
        let roots = tree.as_array().expect("an array of roots");
        assert_eq!(roots.len(), 1, "a well-formed session has exactly one root: {tree}");
        assert_eq!(roots[0]["entry"]["id"], json!(root));
        assert_eq!(roots[0]["label"], json!("checkpoint"), "labels survive the serialization");
        assert!(
            roots[0]["labelTimestamp"].is_string(),
            "SEAM-060's labelTimestamp must not be dropped on this side either: {tree}"
        );
        let kids = roots[0]["children"].as_array().expect("children");
        assert_eq!(kids.len(), 1, "the leaf hangs off the root: {tree}");
        assert_eq!(kids[0]["entry"]["id"], json!(leaf));
    }

    /// EXT-045 — `scoped_models` must report the session's REAL scoped set, in pi's
    /// `ScopedModel` shape (`{model, thinkingLevel?}`, `core/model-resolver.ts:63-67`).
    ///
    /// pi exposes it on the base context (`getScopedModels()`, `core/extensions/runner.ts:706-709`;
    /// `getScopedModels: () => this._scopedModels`, `core/agent-session.ts:2416`), so a guest can
    /// tell a `--models`-scoped session from an unscoped one. Reading `[]` forever made the two
    /// indistinguishable and every model-picking extension free to offer models the user had
    /// deliberately excluded. RED before the fix on the seeded assertion.
    #[test]
    fn scoped_models_reports_the_sessions_real_scoped_set() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // Unscoped is upstream's documented "Empty when no scoping is configured".
        assert_eq!(svc.scoped_models(), json!([]));

        svc.update_scoped_models(vec![
            json!({"model": {"id": "faux-1", "provider": "faux"}, "thinkingLevel": "high"}),
            json!({"model": {"id": "faux-2", "provider": "faux"}}),
        ]);
        let scoped = svc.scoped_models();
        let rows = scoped.as_array().expect("an array");
        assert_eq!(rows.len(), 2, "the whole scoped set reaches the guest: {scoped}");
        assert_eq!(rows[0]["model"]["id"], json!("faux-1"));
        assert_eq!(rows[0]["thinkingLevel"], json!("high"), "pi's per-model thinking level survives");
        assert!(
            rows[1].get("thinkingLevel").is_none(),
            "an unset thinkingLevel is OMITTED, matching an `undefined` field upstream: {scoped}"
        );
    }

    // ---------------------------------------------------- provider OAuth login callbacks (pi) --

    /// `oauth_prompt`/`oauth_select` must reach the live dialog renderer.
    ///
    /// pi wires them to the real interaction — `onPrompt: (prompt) => callbacks.prompt({type:
    /// "text", ...prompt})` and `onSelect: (prompt) => callbacks.prompt({type: "select", ...prompt})`
    /// (`core/provider-composer.ts:245,248`) against `AuthInteraction.prompt(): Promise<string>`,
    /// "returns the entered/selected string (`select` returns the option id)"
    /// (`packages/ai/src/auth/types.ts:152-161`). cyrup's production backend overrode neither, so a
    /// guest-authored provider's interactive `login` could never obtain a value: every prompt came
    /// back "oauth prompt capability not granted" from a capability nothing in the workspace grants,
    /// and every select came back `None` (which a guest reads as the user cancelling).
    ///
    /// RED before the fix on both round-trip assertions. The headless assertions pass either way and
    /// pin that an unattached renderer still yields pi's `noOpUIContext` denial WITHOUT blocking.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oauth_prompt_and_select_round_trip_through_the_ui_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        // Headless (no renderer): the deny defaults, without blocking — pi's `noOpUIContext`.
        assert!(svc.oauth_prompt("paste the callback url", None, false).is_err());
        assert_eq!(svc.oauth_select("pick an account", &json!([])), None);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        // The scripted renderer answers exactly as the TUI's `UiKind::Input`/`UiKind::Select` arms
        // do: a typed string, or the chosen option STRING out of `options`.
        let seen: Arc<Mutex<Vec<(UiKind, String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                seen2
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((req.kind, req.prompt.clone(), req.options.clone()));
                let reply = match req.kind {
                    UiKind::Input => UiReply::Text(Some("pasted-code".to_string())),
                    // Pick the SECOND row, so the id mapped back cannot be an accident of ordering.
                    UiKind::Select => UiReply::Text(
                        req.options.as_array().and_then(|a| a.get(1)).and_then(Value::as_str).map(str::to_string),
                    ),
                    _ => UiReply::Confirm(false),
                };
                let _ = req.reply.send(reply);
            }
        });

        let s1 = svc.clone();
        let prompted = tokio::task::spawn_blocking(move || {
            s1.oauth_prompt("paste the callback url", Some("https://…"), false)
        })
        .await
        .expect("oauth prompt task");
        assert_eq!(
            prompted.as_deref(),
            Ok("pasted-code"),
            "pi's `prompt()` resolves with the entered string, not a capability denial"
        );

        let s2 = svc.clone();
        let picked = tokio::task::spawn_blocking(move || {
            s2.oauth_select(
                "pick an account",
                &json!([
                    {"id": "acct-1", "label": "Personal"},
                    {"id": "acct-2", "label": "Work"},
                ]),
            )
        })
        .await
        .expect("oauth select task");
        assert_eq!(
            picked.as_deref(),
            Some("acct-2"),
            "`select` returns the option ID (auth/types.ts:157), mapped back from the label the \
             renderer displayed"
        );

        // The renderer saw the OAuth selector's LABELS, not raw `{id,label}` objects it cannot
        // render (the `UiRequest.options` contract is a flat array of option strings).
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let select = seen.iter().find(|(k, _, _)| *k == UiKind::Select).expect("a select request");
        assert_eq!(select.2, json!(["Personal", "Work"]), "labels are what reach the renderer");
        assert!(
            seen.iter().any(|(k, p, _)| *k == UiKind::Input && p == "paste the callback url"),
            "the prompt message rides the dialog title, like every other kind: {seen:?}"
        );
    }

    /// `allow_empty: false` is the guest declaring the value mandatory (`world.wit:871`), so an
    /// empty submission is not an answer — pi's prompt rejects rather than resolving with `""`.
    /// With `allow_empty: true` the same submission IS the answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oauth_prompt_honours_allow_empty() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(UiReply::Text(Some(String::new())));
            }
        });

        let s1 = svc.clone();
        let strict = tokio::task::spawn_blocking(move || s1.oauth_prompt("token?", None, false))
            .await
            .expect("task");
        assert!(strict.is_err(), "a mandatory prompt does not resolve with an empty value");

        let s2 = svc.clone();
        let lenient = tokio::task::spawn_blocking(move || s2.oauth_prompt("token?", None, true))
            .await
            .expect("task");
        assert_eq!(lenient.as_deref(), Ok(""), "allow-empty accepts the empty submission");
    }

    /// EXT-037 — the override must (a) answer `None` when no catalog is attached, so the cyrup-ext
    /// binding's registry fallback stays reachable, and (b) pass the live catalog's rows through
    /// UNCHANGED — same order, same `name:N` invocation spelling, same descriptions.
    ///
    /// Scope note: that pass-through is the whole contract of this override. That the rows THEMSELVES
    /// are pi's `[...extensionCommands, ...templates, ...skills]` is `slash_command_catalog`'s
    /// contract, asserted against a real session in `crate::tests::round8_postrun` and
    /// `crate::tests::install_noop`; the double here stands in for it because a `LiveHostServices`
    /// unit test cannot build an `AgentSession`.
    #[test]
    fn commands_passes_the_live_catalog_through_unchanged() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // Unattached: `None` ⇒ the cyrup-ext binding falls back to the registry's resolved commands.
        assert!(svc.commands().is_none(), "no catalog attached ⇒ no live answer");

        svc.attach_session_catalog(Arc::new(FakeCatalog));
        let rows = svc.commands().expect("an attached catalog answers");
        let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
        assert_eq!(
            names,
            ["deploy", "deploy:2", "review", "skill:pdf"],
            "extension commands (with the `name:N` collision spelling), then templates, then skills"
        );
        let sources: Vec<&str> = rows.iter().filter_map(|r| r["source"].as_str()).collect();
        assert_eq!(sources, ["extension", "extension", "prompt", "skill"]);
        assert!(
            rows.iter().all(|r| !r["description"].as_str().unwrap_or_default().is_empty()),
            "every row carries a description — the bare-name walk carried none"
        );
    }

    // ===================================================== TUI-030 / the `custom` seam ==========

    /// TUI-030 — the four working-indicator verbs must reach the mode's effect drain.
    ///
    /// **PRE-FIX this test fails on its first assertion**: `LiveHostServices` overrode none of the
    /// four, so each call took the `HostServices` trait's empty default body, `emit_ui_effect` was
    /// never reached, and `drained` came back EMPTY — `assert_eq!(got.len(), 4)` saw `0`. The test
    /// deliberately drives the four `HostServices` methods on the LIVE backend (not the `UiEffect`
    /// enum, and not a shared helper) precisely because that is the seam that was dead: a test that
    /// constructed the four variants by hand would pass against the unfixed tree.
    #[tokio::test]
    async fn the_working_indicator_family_reaches_the_effect_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
        svc.set_ui_effect_sink(tx);

        svc.set_working_message(Some("indexing the repo"));
        svc.set_working_visible(false);
        svc.set_working_indicator(Some(&json!({"frames": ["-", "\\", "|", "/"], "intervalMs": 120})));
        svc.set_hidden_thinking_label(Some("redacted"));

        let mut got = Vec::new();
        while let Ok(effect) = rx.try_recv() {
            got.push(effect);
        }
        assert_eq!(got.len(), 4, "all four verbs must emit; got {got:?}");
        assert_eq!(
            got[0],
            UiEffect::SetWorkingMessage { message: Some("indexing the repo".to_string()) }
        );
        assert_eq!(got[1], UiEffect::SetWorkingVisible { visible: false });
        assert_eq!(
            got[2],
            UiEffect::SetWorkingIndicator {
                options: Some(json!({"frames": ["-", "\\", "|", "/"], "intervalMs": 120}))
            },
            "the whole `WorkingIndicatorOptions` bag rides through, not just the frames"
        );
        assert_eq!(
            got[3],
            UiEffect::SetHiddenThinkingLabel { label: Some("redacted".to_string()) }
        );

        // `None` is upstream's no-argument call ("restore the default") and must be DISTINGUISHABLE
        // from "never called" — it is a value on the wire, not an absence.
        svc.set_working_message(None);
        svc.set_hidden_thinking_label(None);
        svc.set_working_indicator(None);
        assert_eq!(rx.try_recv().ok(), Some(UiEffect::SetWorkingMessage { message: None }));
        assert_eq!(rx.try_recv().ok(), Some(UiEffect::SetHiddenThinkingLabel { label: None }));
        assert_eq!(rx.try_recv().ok(), Some(UiEffect::SetWorkingIndicator { options: None }));
    }

    /// MIRROR: with NO effect sink (headless print/json) the four silently drop and — critically —
    /// never block, which is Pi's `noOpUIContext` (`core/extensions/runner.ts:242-245` @v0.84.2,
    /// four `() => {}` bodies). A single-thread runtime proves no `block_in_place` is reached.
    #[test]
    fn the_working_indicator_family_is_a_silent_no_op_without_a_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        svc.set_working_message(Some("nobody is listening"));
        svc.set_working_visible(true);
        svc.set_working_indicator(None);
        svc.set_hidden_thinking_label(Some("x"));
    }

    /// SEAM — `custom` must reach a real interactive surface for a WASM guest.
    ///
    /// **PRE-FIX this test fails on its first assertion**: `LiveHostServices` did not override
    /// `custom` at all, so it took the trait default `None`. The scripted renderer below would
    /// never receive an `OverlayRequest` (`took_it` stays `false`) and the returned value would be
    /// `None` instead of the chosen option id. Nothing else in the tree could have made it pass:
    /// `open_overlay` — the NATIVE tier's route, which was always implemented — is only reached
    /// here because the fix routes the guest's spec onto it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn custom_drives_a_guest_spec_through_the_overlay_renderer() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OverlayRequest>();
        svc.set_overlay_sink(tx);

        // The scripted renderer: paint once (proving the spec really became a driveable component),
        // press Down then Enter (the human choosing the SECOND row), then tear the modal down.
        let painted: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let painted2 = Arc::clone(&painted);
        tokio::spawn(async move {
            while let Some(mut req) = rx.recv().await {
                let rows = req.overlay.render(60, 24);
                *painted2.lock().unwrap_or_else(|e| e.into_inner()) =
                    rows.iter().map(cyrup_ext::host::OverlayLine::plain_text).collect();
                req.overlay.handle_key(cyrup_ext::host::OverlayKey::plain(
                    cyrup_ext::host::OverlayKeyCode::Down,
                ));
                req.overlay.handle_key(cyrup_ext::host::OverlayKey::plain(
                    cyrup_ext::host::OverlayKeyCode::Enter,
                ));
                // Dropping `req.overlay` here would be the renderer closing the modal; the caller
                // is released by the `done` one-shot, exactly as the TUI's run loop releases it.
                let _ = req.done.send(());
            }
        });

        let s = Arc::clone(&svc);
        let picked = tokio::task::spawn_blocking(move || {
            s.custom(&json!({
                "title": "Pick a target",
                "lines": ["two hosts are reachable"],
                "options": ["staging", {"id": "prod", "label": "production (careful)"}],
            }))
        })
        .await
        .expect("the custom task");

        assert_eq!(
            picked.as_deref(),
            Some("prod"),
            "the chosen row's id comes back to the guest, not its label"
        );
        let rows = painted.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            rows,
            vec![
                "Pick a target".to_string(),
                "two hosts are reachable".to_string(),
                "> staging".to_string(),
                "  production (careful)".to_string(),
            ],
            "the guest's spec really rendered — title, body, then the options with a gutter marker"
        );
    }

    /// MIRROR: with NO overlay renderer (headless print/json, and RPC — whose wire cannot stream
    /// keystrokes into a host component) `custom` answers `None` WITHOUT blocking, which is pi's own
    /// RPC body verbatim (`async custom() { return undefined as never }`,
    /// `modes/rpc/rpc-mode.ts:228-231` @v0.84.2). A single-thread runtime proves the non-blocking part: a
    /// `block_in_place` would panic here.
    #[test]
    fn custom_declines_without_an_overlay_renderer_and_without_blocking() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        assert_eq!(svc.custom(&json!({"title": "hi", "options": ["a"]})), None);
        // …and an empty/garbage spec is declined even WITH a renderer, rather than opening a blank
        // modal a human has to dismiss.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<OverlayRequest>();
        svc.set_overlay_sink(tx);
        assert_eq!(svc.custom(&json!({})), None);
        assert_eq!(svc.custom(&Value::Null), None);
    }
}

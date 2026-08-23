//! The `ui.*` seam's wire types: the request/reply dialog carrier ([`UiRequest`] / [`UiReply`]), the
//! interactive-overlay handoff ([`OverlayRequest`]), the fire-and-forget effect enum ([`UiEffect`]),
//! the three sinks a mode installs to service them, and the editor-buffer mirror
//! ([`EditorTextMirror`]) the `editor-text` read answers from.
//!
//! Plain carriers with no behaviour of their own (beyond the mirror's two accessors): the backend
//! that owns the sinks and performs the round trips is [`LiveHostServices`], in this module's
//! `mod.rs`.

use std::sync::{Arc, Mutex};

use cyrup_ext::host::{DialogOptions, InteractiveOverlay, NotifyKind};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

// Doc-only: the docs below name the backend that owns these carriers, its fire-and-forget sibling
// sink, and the trait methods they back. Nothing in this file names any of the three in code.
#[cfg(doc)]
use super::{ControlSink, LiveHostServices};
#[cfg(doc)]
use cyrup_ext::host::HostServices;


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
/// [`LiveHostServices`]'s effect methods silently drop (== Pi `noOpUIContext`'s `notify`/`setStatus`/… no-ops,
/// runner.ts:234-244).
pub type UiEffectSink = UnboundedSender<UiEffect>;

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
        *crate::sync::lock(&self.0) = text.into();
    }

    /// The current extension-visible buffer text.
    pub fn text(&self) -> String {
        crate::sync::lock(&self.0).clone()
    }
}

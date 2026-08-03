//! The app shell + event loop (arch-10 §3.3 `Tui`, §5 concurrency; R-ARCH-TUI-003/004).
//!
//! [`App`] is generic over `ratatui::backend::Backend` so the same render/ingest/input logic runs
//! against a real terminal (`CrosstermBackend`) and a headless `TestBackend` (R-10-010 /
//! R-ARCH-TUI-010). The interactive layout is an **inline viewport** (NOT the alternate screen,
//! R-ARCH-TUI-003): the live region holds only the *active* region — the in-flight streaming turn,
//! the editor, and the status line. Each committed conversation entry is flushed exactly once to the
//! terminal's native scrollback via `Terminal::insert_before` ([`App::draw`] →
//! `flush_committed`), so completed history scrolls natively and is never re-rendered in the viewport.
//!
//! `render` is pure (`state -> frame`): [`render`] takes `&mut AppState` and a `Frame` and never
//! touches real I/O, so tests draw into a `TestBackend` buffer and assert on cells.

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{CancelToken, EventStream, ModelThinkingLevel};
// The extension-facing session backend trait: brings `LiveHostServices::set_label` (the live
// label-append the `/tree` `e` rename persists through — the SAME path a guest's `setLabel` uses,
// host_services.rs:866) into scope.
use cyrup_ext::host::HostServices;
use cyrup_provider::StreamEvent;
use cyrup_resources::theme::ThemeData;
use cyrup_session_svc::{AgentSession, AgentSessionEvent, CompactionReason, InputSource, UserInput};
use cyrup_session_svc::{
    AgentSessionRuntime, ForkPosition, NavigateTreeOptions, SessionDagKind, SessionDagNode,
};
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use futures::StreamExt;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, Clear, ClearType,
    EndSynchronizedUpdate,
};
use ratatui::crossterm::{execute, queue, ExecutableCommand};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::commands::{CommandRegistry, Dispatch};
use crate::component::{Component, InputEvent};
use crate::editor::{EditorOutcome, InputEditor};
use crate::error::TuiError;
use crate::extension_editor::ExtensionEditorSelector;
use crate::image::{ImageBlock, ImageRenderer, TerminalCapabilities};
use crate::keymap::{Action, EditorAction, Key, Keymap, SelectKeymap, TreeKeymap};
use crate::model_selector::{ModelEntry, ModelSelector};
use crate::overlay::{HotkeyRow, HotkeysOverlay, Overlay, OverlayOutcome};
use crate::selector::{
    CheckboxSelector, ListSelector, Selector, SelectorKind, SelectorOutcome,
};
use crate::session_selector::{SessionRow, SessionSelector, SessionSelectorOutcome};
use crate::settings_selector::{SettingRow, SettingsSelector, TrustSelector};
use crate::status::StatusLine;
use crate::status_indicator::{IndicatorKind, StatusIndicator, SPINNER_INTERVAL};
use crate::text_input::TextInputSelector;
use crate::theme::{ColorMode, ThemeController, UiTheme};
use crate::transcript::{content_text, entry_lines, thinking_text, TranscriptView};
use crate::tree_selector::{TreeKind, TreeNode, TreeSelector};

/// The number of visual lines a `PageUp`/`PageDown` scrolls the active region by (a conservative
/// screenful; spec/tui/07 page-scroll). Resolved on the pure input thread without the live viewport
/// height, then clamped against the real content at render time.
const PAGE_SCROLL_LINES: usize = 10;

/// The decision produced by feeding one input event to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    /// The user submitted a prompt (already optimistically shown in the transcript).
    Submit(String),
    /// The user queued the editor text as a follow-up (Alt+Enter, `app.message.followUp`): the run
    /// loop delivers it via [`AgentSession::follow_up`] when a turn is streaming, or as a plain submit
    /// when idle (Pi `handleFollowUp`, interactive-mode.ts:3554-3585). Carries the trimmed text.
    FollowUp(String),
    /// The user asked to restore queued messages back into the editor (Alt+Up, `app.message.dequeue`):
    /// the run loop reads the session's steering + follow-up queues, clears them, and prepends their
    /// text to the current editor buffer (Pi `handleDequeue` → `restoreQueuedMessagesToEditor`,
    /// interactive-mode.ts:3587-3594,3852-3871). Needs the live session (async queue read/clear), so
    /// like [`Self::FollowUp`] it rides an `AppAction` the run loop resolves.
    Dequeue,
    /// The user requested an abort/interrupt of the in-flight run (Esc).
    Interrupt,
    /// Esc pressed **while a turn is streaming** (Pi `defaultEditor.onEscape` first branch,
    /// interactive-mode.ts:2636-2637 → `restoreQueuedMessagesToEditor({abort: true})`).
    ///
    /// Distinct from [`Self::Interrupt`] because pi does not merely abort here: it first take-alls
    /// BOTH pending queues and puts their text back into the editor, so steering / follow-up
    /// messages the user typed during the run survive the interrupt instead of being silently
    /// dropped. The run loop drains ([`AgentSession::drain_queue`]), hands the result to
    /// [`App::restore_queued_to_editor`], and only then aborts.
    InterruptRestoreQueued,
    /// The user requested to quit the session.
    Quit,
    /// The user requested to suspend the process to the background (Ctrl+Z / SIGTSTP). The run loop
    /// tears down raw mode, raises `SIGTSTP`, and re-enters raw mode on `SIGCONT`.
    Suspend,
    /// A `!`/`!!` bash invocation: the run loop spawns the shell command, streams its output into the
    /// live bash block, and (for `!`, not `!!`) feeds the result back into the session context
    /// (`bash-execution.ts`; interactive-mode.ts `!` handler).
    RunBash { command: String, excluded: bool },
    /// Open the editor buffer in `$VISUAL`/`$EDITOR` (Ctrl+G, `app.editor.external`): the run loop
    /// tears the terminal down, launches the editor on a temp file, then reloads the buffer
    /// (`openExternalEditor`, interactive-mode.ts:3611).
    OpenExternalEditor,
    /// `Ctrl+G` pressed inside an open extension `ui.editor` dialog (L4 review §3): same teardown as
    /// [`Self::OpenExternalEditor`], but seeded from — and written back into — the dialog's OWN
    /// buffer rather than [`AppState::editor`] (Pi `ExtensionEditorComponent.openExternalEditor`,
    /// `extension-editor.ts:119-157`).
    OpenExternalEditorForSelector,
    /// A recognized slash command whose effect lives at the session/data layer (`setupEditorSubmitHandler`,
    /// interactive-mode.ts:2549-2734). The run loop executes it against the [`AgentSession`] (open a
    /// data-bound selector after sourcing its rows, drive the session lifecycle, export, copy, …).
    Command(AppCommand),
    /// A registered extension keyboard shortcut fired (R-08-017; Pi `registerShortcut`). Carries the
    /// shortcut key-id; the run loop dispatches it to the session's extension host
    /// (`ExtensionHost::run_shortcut` → `LiveExtension::execute_shortcut`).
    ExtensionShortcut(String),
    /// State changed; the frame should be redrawn.
    Redraw,
    /// Nothing to do.
    None,
}

/// The direction the model-cycle keybindings move through the cycle set (`app.model.cycleForward` vs
/// `cycleBackward`, `core/keybindings.ts:76-83`; Pi `cycleModel(direction)`, interactive-mode.ts:3617).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleDirection {
    /// Next model (Ctrl+P).
    Forward,
    /// Previous model (Shift+Ctrl+P).
    Backward,
}

/// A slash command / keybinding whose execution the run loop performs against the session/resources
/// layer (the in-crate effects — `/hotkeys`, `/debug`, `/changelog`, `/quit` — are applied directly in
/// [`App::dispatch_submission`] and never become an `AppCommand`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppCommand {
    /// Open a data-bound selector; the run loop sources its rows from session-svc / resources.
    OpenSelector(SelectorKind),
    /// `/model [text]` (`handleModelCommand`, interactive-mode.ts:4175-4196): bare (`None`) opens the
    /// unfiltered picker; a `Some(text)` that EXACTLY matches a model sets it directly (no picker),
    /// otherwise opens the picker pre-filtered to `text`. The run loop resolves the exact-match against
    /// the live catalog (`findExactModelReferenceMatch`), so the search term rides the command.
    ModelCommand(Option<String>),
    /// Persist an entry's `/tree` label (Pi `onLabelChange` → `sessionManager.appendLabelChange`,
    /// interactive-mode.ts:4589-4591): set/replace when `label` is non-empty, remove when empty
    /// (`apply_label` drops empty labels). The run loop applies it via the session's `set_label` path.
    SetEntryLabel { entry_id: String, label: String },
    /// Cycle to the next/previous model in place (`app.model.cycleForward`/`cycleBackward`): the run
    /// loop reads the cycle set (the scoped models if any, else the available catalog), advances from
    /// the current model, and calls [`AgentSession::set_model`] (Pi `cycleModel`,
    /// interactive-mode.ts:3617-3632).
    CycleModel(CycleDirection),
    /// Cycle the reasoning level one step (`app.thinking.cycle`, Shift+Tab): the run loop calls
    /// [`AgentSession::cycle_thinking_level`], which is GATED on model support — a non-reasoning model
    /// returns `None` (nothing changes) and cycling otherwise walks the model's OWN supported levels
    /// (incl. `xhigh` where mapped), exactly like Pi's `cycleThinkingLevel` (agent-session.ts:1599 →
    /// interactive-mode.ts:3606-3614). Rides an `AppCommand` because the gate needs the live model.
    CycleThinking,
    /// Apply a confirmed data-bound selection (`{kind}` chose `{value}`): set the model, switch the
    /// branch, login/logout, etc.
    ConfirmSelection { kind: SelectorKind, value: String },
    /// Persist a settings field changed **in place** in the `/settings` grid (Pi settings-selector
    /// `onChange` → `SettingsManager.setNested`). The slot stays open; the `/reload` re-reads it.
    ApplySetting { id: String, value: String },
    /// `/new` — start a fresh session (`handleClearCommand`).
    NewSession,
    /// `/compact [instructions]` — manually compact context (`handleCompactCommand`).
    Compact(Option<String>),
    /// `/clone` — duplicate the session at the current position (`handleCloneCommand`).
    Clone,
    /// `/reload` — rebuild keybindings/extensions/skills/prompts/themes (`handleReloadCommand`).
    Reload,
    /// `/export [path]` — export the session (`handleExportCommand`).
    Export(Option<String>),
    /// `/import <path>` — import + resume a JSONL session (`handleImportCommand`).
    Import(Option<String>),
    /// `/share` — publish the session as a secret gist (`handleShareCommand`).
    Share,
    /// `/copy` — copy the last assistant message to the clipboard (`handleCopyCommand`).
    Copy,
    /// `/name <name>` — set the session display name (`handleNameCommand`).
    SetName(String),
    /// `/session` — show session info + stats (`handleSessionCommand`).
    SessionInfo,
    /// `/resume` in-list delete of a persisted session file (session-selector.ts:540 →
    /// `delete_session_file`). Carries the session path.
    DeleteSession(String),
    /// `/resume` in-list rename of a persisted session (session-selector.ts:585 →
    /// `rename_session_file`). Carries the session path + new name.
    RenameSession { path: String, name: String },
}

/// All retained UI state (the data half of the `state -> frame` split).
pub struct AppState {
    pub transcript: TranscriptView,
    pub editor: InputEditor,
    pub status: StatusLine,
    /// The working/idle status band (spec/tui/01 §6) — a 2-row spinner+message while a turn/retry/
    /// compaction runs, blank when idle. Driven by `AgentSessionEvent`s in [`App::ingest_event`].
    pub indicator: StatusIndicator,
    pub theme: UiTheme,
    /// The terminal color depth every theme is projected into (feature #3/#4): boot resolves it from
    /// the terminal (`ColorMode::detect` / the `ThemeController`); a live `/theme` switch re-projects
    /// the new theme through it so 256-color terminals keep indexed colors after a switch.
    pub color_mode: ColorMode,
    pub keymap: Keymap,
    /// The selector binding table (`tui.select.*`, spec/tui/05 §10) consulted while a selector owns
    /// the input slot.
    pub select_keymap: SelectKeymap,
    /// The `/tree` bespoke binding table (`app.tree.*`, spec/tui/05 §6.1) handed to each opened
    /// [`TreeSelector`] so JSON rebinds of fold/unfold/label flow through (R-10-018).
    pub tree_keymap: TreeKeymap,
    /// The slash-command registry driving dispatch + autocomplete (rebuilt on `/reload`).
    pub commands: CommandRegistry,
    /// The active editor-swap selector, if any (spec/tui/05 §1.1): when `Some`, it replaces the
    /// editor in the bottom inline region and captures input until it confirms/cancels.
    pub selector: Option<ActiveSelector>,
    /// The floating overlay z-stack (spec/tui/05 §2): hotkeys/help popup (and, later, extension UI).
    /// The topmost overlay captures input; rendered over the live region bottom→top.
    pub overlays: Vec<Box<dyn Overlay>>,
    /// The current reasoning level (`off`…`xhigh`), preselected by the thinking selector and updated
    /// on confirm. The authoritative level lives on the agent/session at the L7 layer.
    pub thinking_level: String,
    /// Whether inline images are shown (vs. a text placeholder), toggled by the show-images selector.
    pub show_images: bool,
    /// The terminal image-protocol renderer (spec/tui/06 §6; `terminal-image.ts`). Defaults to the
    /// portable half-block raster; the production binary upgrades it to the real protocol via
    /// [`App::detect_image_support`]. Drives the inline render of [`AppState::pending_images`].
    pub image_renderer: ImageRenderer,
    /// Images attached to the next prompt (the `@`-mention of an image file / a paste), rendered
    /// inline above the editor in the live region (`components/image.ts`), honoring `show_images`.
    pub pending_images: Vec<ImageBlock>,
    /// Reserve the 2-row status band even when idle (spec/tui/01 §6.3). Default `false` (Pi's
    /// non-`clearOnShrink` behavior) so the editor/footer never reflow on idle viewports.
    pub reserve_status_rows: bool,
    /// Whether the compact startup keybinding-hints bar is shown (Pi `compactInstructions`,
    /// interactive-mode.ts:697-703): a one-line `interrupt · clear/exit · / commands · ! bash · more`
    /// affordance bar rendered just above the editor at startup, dismissed on the first submission.
    pub show_startup_hints: bool,
    /// A `DynamicBorder` loader occupying the editor slot during a long inline op (Pi
    /// `BorderedLoader`, bordered-loader.ts): `/share`'s gist creation and any extension-UI long op.
    /// When `Some`, it replaces the editor in the live region (the selector still wins if both are set,
    /// which never happens). Cleared when the op completes.
    pub loader: Option<crate::chrome::BorderedLoader>,
    /// The 80 ms phase index for the active [`Self::loader`] / status spinner (advanced by the run-loop
    /// tick). Drives the loader's animated glyph without a timer thread.
    pub loader_tick: usize,
    /// Set when the user requested quit; the run loop observes it.
    pub should_quit: bool,
    /// Timestamp of the last `Ctrl+C` press, for the double-tap-to-exit gate (Pi `handleCtrlC`,
    /// interactive-mode.ts:3361-3369): a second `Ctrl+C` within 500 ms exits; otherwise it clears the
    /// editor and records the press time. `None` until the first press.
    last_sigint: Option<std::time::Instant>,
    /// A status line to show **after** the next runtime session-swap re-binds the UI (the swap
    /// resets the transcript, so a pre-swap status would be wiped). Set by the session-lifecycle
    /// command handlers (`/new`/`/resume`/`/fork`/`/reload`/`/import`); consumed by
    /// [`App::rebind_session`] once the generation bump fires and the new session is installed.
    pub pending_swap_status: Option<String>,
    /// Committed lines already emitted to native scrollback via `Terminal::insert_before`
    /// (R-ARCH-TUI-003). Kept as a test-visible accumulator mirroring exactly what was handed to
    /// `insert_before`; never re-rendered inside the inline viewport.
    pub scrollback: Vec<Line<'static>>,
    /// Extension-registered keyboard shortcuts (R-08-017; Pi `registerShortcut`): each parsed
    /// [`Key`] spec paired with the raw key-id string the host routes on. Sourced from
    /// `ExtensionHost::shortcut_keys()` at boot and refreshed on session swap; a matching key press
    /// (checked at the global-keymap tier, after built-in bindings) becomes an
    /// [`AppAction::ExtensionShortcut`]. Empty when no extension registered a shortcut.
    pub extension_shortcuts: Vec<(Key, String)>,
    /// The env-sniffed terminal capabilities (feature #7/#8; Pi `getCapabilities`): image protocol +
    /// truecolor + OSC-8-hyperlink forwarding. Boot default is conservative (half-block, no
    /// hyperlinks); the binary refines it via [`App::detect_image_support`]. The `hyperlinks` flag
    /// gates OSC-8 emission in rendered links (`osc::hyperlink`).
    pub capabilities: TerminalCapabilities,
    /// The REPLY half of the extension-UI dialog currently occupying [`Self::selector`] (`kind ==
    /// SelectorKind::Extension{Confirm,Select,Input}`), if any (L4 review §2.1). A loaded guest's
    /// synchronous `ui.{confirm,select,input}` call blocks its own tokio task on this one-shot
    /// (`LiveHostServices::ui_roundtrip`) until the selector confirms or cancels; `App::run`'s `ui_rx`
    /// arm sets it when it opens the dialog, and [`App::confirm_selector`] /
    /// [`App::handle_selector_key`]'s `Cancel` arm take + resolve it. `None` whenever no extension
    /// dialog is open (including every ordinary first-party selector).
    pending_ui_reply: Option<PendingUiReply>,
}

impl AppState {
    /// Fresh state with the given theme.
    pub fn new(theme: UiTheme) -> Self {
        AppState {
            transcript: TranscriptView::new(),
            editor: InputEditor::new(),
            status: StatusLine::default(),
            indicator: StatusIndicator::new(),
            color_mode: theme.color_mode,
            theme,
            keymap: Keymap::default(),
            select_keymap: SelectKeymap::default(),
            tree_keymap: TreeKeymap::default(),
            commands: CommandRegistry::new(),
            selector: None,
            overlays: Vec::new(),
            thinking_level: "medium".to_string(),
            show_images: true,
            image_renderer: ImageRenderer::default(),
            pending_images: Vec::new(),
            reserve_status_rows: false,
            show_startup_hints: true,
            loader: None,
            loader_tick: 0,
            should_quit: false,
            last_sigint: None,
            pending_swap_status: None,
            scrollback: Vec::new(),
            extension_shortcuts: Vec::new(),
            capabilities: TerminalCapabilities {
                images: None,
                true_color: true,
                hyperlinks: false,
            },
            pending_ui_reply: None,
        }
    }

    /// Install the extension-registered keyboard shortcuts (R-08-017): each raw key-id is parsed to a
    /// [`Key`] spec (unparseable ids are dropped, never panicking) and retained with its id so a
    /// matching press routes to the owning extension. Called by the binary at boot and after a
    /// session swap, so a `/reload` that changes the registered set takes effect.
    pub fn set_extension_shortcuts(&mut self, key_ids: impl IntoIterator<Item = String>) {
        self.extension_shortcuts = key_ids
            .into_iter()
            .filter_map(|id| Key::parse(&id).ok().map(|k| (k, id)))
            .collect();
    }
}

/// The active editor-swap selector plus the state needed to restore the editor on close (spec/tui/05
/// §7 `ActiveSelector`). Pi snapshots the editor text on open (`interactive-mode.ts:2371`) and, for
/// the theme picker, restores the prior theme on cancel (`theme-selector.ts` caller responsibility).
pub struct ActiveSelector {
    kind: SelectorKind,
    inner: Box<dyn Selector>,
    /// Editor text snapshotted on open, re-applied when the slot closes.
    saved_editor: String,
    /// Theme to restore if a previewing selector is cancelled (theme picker only).
    restore_theme: Option<UiTheme>,
}

/// The REQUEST/REPLY pairing an open extension-UI dialog (`SelectorKind::Extension{Confirm,Select,
/// Input}`) resolves against (L4 review §2.1): `kind` is retained so a `Cancel` can resolve to the
/// correct per-kind deny default ([`default_ui_reply`]) without re-deriving it from the selector kind
/// at the call site.
struct PendingUiReply {
    kind: UiKind,
    reply: tokio::sync::oneshot::Sender<UiReply>,
    /// The dialog's title WITHOUT any countdown suffix, so each tick recomputes `"{base_title}
    /// ({s}s)"` fresh off the current remaining time rather than accumulating appended text.
    base_title: String,
    /// When this dialog auto-resolves to its per-kind deny default if the user hasn't answered by
    /// then — Pi's `CountdownTimer` (`countdown-timer.ts:7-38`), armed from the guest's
    /// `opts.timeout_ms` exactly like `LiveHostServices::ui_roundtrip`'s OWN independent host-side
    /// timeout race (`host_services.rs`) arms from the SAME value; the two are deliberately separate
    /// clocks (mirroring Pi's `createDialogPromise`'s host-armed `setTimeout` vs. the renderer's own
    /// `CountdownTimer`, `rpc-mode.ts:114-119`) — whichever fires first wins the reply, and the loser
    /// finds it a harmless no-op. `None` when the guest set no timeout (dialog waits indefinitely for
    /// a key, matching `ui_roundtrip`'s own `None` branch).
    deadline: Option<tokio::time::Instant>,
}

/// The per-kind deny default a dialog resolves to when the user cancels it (`Esc`) rather than
/// answering — Pi's `noOpUIContext` shape (`runner.ts:230-261`), the same mapping
/// `crates/cyrup-modes/src/rpc.rs`'s `default_ui_reply` uses for a timed-out/force-resolved RPC dialog.
fn default_ui_reply(kind: UiKind) -> UiReply {
    match kind {
        UiKind::Confirm => UiReply::Confirm(false),
        UiKind::Input | UiKind::Editor | UiKind::Select => UiReply::Text(None),
    }
}

/// Format `base` with a live "(Ns)" countdown suffix — Pi's `CountdownTimer`'s exact title format
/// (`` `${this.baseTitle} (${s}s)` ``, `countdown-timer.ts:14,23,55`). Rounds UP (Pi's own
/// `Math.ceil(timeoutMs / 1000)`, `countdown-timer.ts:18`) so e.g. 4500ms remaining reads "5s", not
/// "4s"; a `deadline` already in the past reads "0s" (the tick loop closes the dialog that same
/// pass, so this is never rendered for more than one frame).
fn countdown_title(base: &str, deadline: tokio::time::Instant) -> String {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let secs = remaining.as_millis().div_ceil(1000);
    format!("{base} ({secs}s)")
}

/// A backend that can rebuild a fresh handle over the **same** underlying terminal, so the inline
/// viewport can be re-sized to the live region's content height (ratatui's `Viewport::Inline` height
/// is fixed at construction; ADR-0001 commitment #1 / audit #1 require a content-sized region). The
/// rebuilt backend must preserve the cursor anchor used to place the inline viewport: `TestBackend`
/// copies its grid cursor; `CrosstermBackend` re-wraps `stdout`, where the real terminal cursor is
/// already authoritative.
pub trait RebuildBackend: Backend + Sized {
    /// A fresh backend over the same terminal, preserving the inline-viewport cursor anchor.
    fn rebuild(&self) -> Self;

    /// Prepare the **current** backend for a reconstruction at `new_height` (called by
    /// [`App::draw`] → `resize_viewport` immediately before [`rebuild`](Self::rebuild)): erase the
    /// current inline region and re-anchor the cursor so the rebuilt viewport leaves **no residual
    /// chrome** on a real terminal — the fix for the inline-viewport STACKING bug. See
    /// [`reanchor_inline_region`] for the geometry.
    ///
    /// The default is a **no-op**, which is correct for fresh-grid backends whose `rebuild` starts
    /// from a blank buffer and re-anchors itself (e.g. `TestBackend`): they can never stack, so there
    /// is nothing to erase. Only a persistent real-screen backend (`CrosstermBackend`) needs it.
    fn reanchor_inline(&mut self, _term_height: u16, _old_height: u16, _new_height: u16) {}
}

/// Erase the current inline region and re-anchor the cursor so that reconstructing the inline
/// viewport at `new_height` leaves **no residual chrome** (the inline-viewport STACKING fix).
///
/// ratatui's `Viewport::Inline(height)` is immutable after construction and `Terminal::resize` cannot
/// change it (it treats its argument as the *terminal* size), so a content-sized live region must be
/// reconstructed whenever its height changes. Reconstruction reserves the new rows by calling
/// `Backend::append_lines`, which — at the bottom of a real terminal — SCROLLS: without this step the
/// prior frame's hint bar / editor rules / footer scroll up into native scrollback and stay visible,
/// so a streaming turn that grows over several frames leaves a stack of duplicated chrome.
///
/// Emitting `MoveTo(top-of-current-region)` + `Clear(FromCursorDown)` first means that scroll carries
/// **blanks**, not chrome. The returned row is where the cursor is left; the reconstruction's
/// `compute_inline_size` reads it via `get_cursor_position`. The anchor is bottom-aligned with the
/// **minimal** scroll — `term_height - min(old, new)`: on growth this is the old region's top (append
/// scrolls exactly the delta); on shrink it sits lower so the shorter region stays pinned to the
/// bottom with no scroll. Both cases leave the region bottom-anchored, for growth **and** shrink.
pub fn reanchor_inline_region<W: io::Write>(
    w: &mut W,
    term_height: u16,
    old_height: u16,
    new_height: u16,
) -> u16 {
    let term_h = term_height.max(1);
    let last_row = term_h.saturating_sub(1);
    let anchor_y = term_h.saturating_sub(old_height.min(new_height)).min(last_row);
    if old_height > 0 {
        // Erase the whole current inline region (its top row down to the bottom of the screen).
        let erase_top = term_h.saturating_sub(old_height).min(last_row);
        let _ = queue!(w, MoveTo(0, erase_top), Clear(ClearType::FromCursorDown));
    }
    let _ = queue!(w, MoveTo(0, anchor_y));
    let _ = w.flush();
    anchor_y
}

impl RebuildBackend for ratatui::backend::TestBackend {
    fn rebuild(&self) -> Self {
        let area = self.buffer().area;
        let mut next = ratatui::backend::TestBackend::new(area.width, area.height);
        // Anchor the inline viewport at the **bottom** of the screen, exactly as a real terminal does
        // at launch (the cursor sits after the shell prompt). This makes `insert_before` scroll
        // committed history up off the top into native scrollback (out of the visible buffer) instead
        // of leaving it on-screen above a top-anchored viewport (ADR-0001; audit #1).
        let bottom = ratatui::layout::Position { x: 0, y: area.height.saturating_sub(1) };
        let _ = Backend::set_cursor_position(&mut next, bottom);
        next
    }
}

impl RebuildBackend for CrosstermBackend<Stdout> {
    fn rebuild(&self) -> Self {
        // The real terminal cursor is authoritative; a fresh wrapper over stdout re-reads it.
        CrosstermBackend::new(io::stdout())
    }

    fn reanchor_inline(&mut self, term_height: u16, old_height: u16, new_height: u16) {
        // `CrosstermBackend<W>: io::Write` — emit the erase + re-anchor straight to stdout, so the
        // next `rebuild()` + `Terminal::with_options` reserves the new rows without stacking chrome.
        let _ = reanchor_inline_region(self, term_height, old_height, new_height);
    }
}

/// The interactive front-end over an injectable backend.
pub struct App<B: Backend> {
    terminal: Terminal<B>,
    state: AppState,
    /// The current inline-viewport height (the live region's content height). Recomputed each
    /// [`draw`](Self::draw); the viewport is rebuilt only when it changes (audit #1).
    viewport_height: u16,
    /// Grow-only high-water mark for the live-region height WHILE a turn is active (streaming or a
    /// live `!` bash block). During a turn the viewport pins at this floor and stops tracking
    /// per-tool content churn, so the terminal is reconstructed (`resize_viewport` → `reanchor_inline`)
    /// only on GENUINE geometry changes (terminal resize, editor multi-line growth, selector/overlay/
    /// band) and the two idle↔active transitions per turn — never per completed tool. That stable
    /// height is what lets ratatui cell-diff the message churn inside a fixed viewport with no full
    /// repaint, eliminating the per-tool-call FLICKER. Reset to `0` the instant the turn goes idle so
    /// the region collapses back to the compact editor/footer (the void-fix is preserved).
    live_floor: u16,
}

impl<B: Backend> App<B> {
    /// Build an app over `backend` using a **content-sized inline viewport** (R-ARCH-TUI-003,
    /// ADR-0001 #1): the live region holds only the active turn + status band + editor/selector +
    /// footer, so finished history flushes to native scrollback (`insert_before`) instead of the
    /// inline region swallowing the whole screen. No alternate screen is entered.
    pub fn new(backend: B, theme: UiTheme) -> Result<Self, TuiError> {
        let size = backend.size().map_err(|e| TuiError::Backend(e.to_string()))?;
        let state = AppState::new(theme);
        let height = live_region_height(&state, size.width, size.height.max(1));
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        // Seed `0` so the first `draw` always rebuilds the viewport bottom-anchored (the constructed
        // `Terminal` is top-anchored at the backend's initial cursor; the rebuild fixes the anchor).
        Ok(App { terminal, state, viewport_height: 0, live_floor: 0 })
    }

    /// Immutable state access.
    pub fn state(&self) -> &AppState {
        &self.state
    }
    /// Mutable state access (drive the transcript/editor/status directly).
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }
    /// Install the extension-registered keyboard shortcuts (R-08-017; delegates to
    /// [`AppState::set_extension_shortcuts`]). The binary calls this at boot from
    /// `ExtensionHost::shortcut_keys()`.
    pub fn set_extension_shortcuts(&mut self, key_ids: impl IntoIterator<Item = String>) {
        self.state.set_extension_shortcuts(key_ids);
    }

    /// Plumb the `autocompleteMaxVisible` setting (Pi, item #6) into the editor's autocomplete popup
    /// (clamped 3–20). The binary calls this from `settings.autocompleteMaxVisible` at boot.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.state.editor.set_autocomplete_max_visible(n);
    }

    /// Whether the idle 2-row status band is reserved (kept present) to avoid an editor/footer reflow
    /// when a spinner appears (item #9). Plumbed from Pi's `terminal.clearOnShrink` setting
    /// (interactive-mode.ts:1638-1642: an idle status container is cleared only when clearOnShrink is
    /// off — so `reserve_status_rows == clearOnShrink`). Default `false` matches Pi's default.
    pub fn set_reserve_status_rows(&mut self, reserve: bool) {
        self.state.reserve_status_rows = reserve;
    }

    /// Load a user `keybindings.json` document and merge it into every live keymap (R-10-018; Pi
    /// `KeybindingsManager.create`, keybindings.ts:348-352). Each map's `merge_json` applies only the
    /// ids in its own namespace (`app.*` / `editor.*` / `tui.select.*` / `app.tree.*`) and ignores the
    /// rest, so one document configures the global, editor, selector and tree maps in a single pass.
    /// A malformed document or key spec is surfaced as a typed error (the binary logs + continues with
    /// the defaults) — never a panic.
    pub fn load_keybindings_json(&mut self, json: &str) -> Result<(), TuiError> {
        self.state.keymap.merge_json(json)?;
        self.state.select_keymap.merge_json(json)?;
        self.state.tree_keymap.merge_json(json)?;
        self.state.editor.merge_keybindings_json(json)?;
        Ok(())
    }
    /// The transcript view.
    pub fn transcript_mut(&mut self) -> &mut TranscriptView {
        &mut self.state.transcript
    }
    /// The input editor.
    pub fn editor_mut(&mut self) -> &mut InputEditor {
        &mut self.state.editor
    }
    /// The status line.
    pub fn status_mut(&mut self) -> &mut StatusLine {
        &mut self.state.status
    }
    /// The terminal (test access to the rendered buffer via `terminal.backend()`).
    pub fn terminal(&self) -> &Terminal<B> {
        &self.terminal
    }

    /// The committed scrollback lines already emitted via `insert_before` (test/inspection access).
    pub fn scrollback_lines(&self) -> &[Line<'static>] {
        &self.state.scrollback
    }

    /// The current inline-viewport (live-region) height in rows — the bottom band of the screen the
    /// app repaints each frame (ADR-0001 #1). Committed history scrolls *above* this band into native
    /// scrollback; tests use it to read only the live region (the bottom `viewport_height` rows).
    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    /// The committed scrollback content as text, one entry per line (test/inspection access). This is
    /// the exact payload `Terminal::insert_before` received, so tests can assert finalized turns
    /// reached native scrollback without driving a real terminal.
    pub fn scrollback_text(&self) -> String {
        self.state.scrollback.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    /// Attach a decoded image to the next prompt (rendered inline above the editor, spec/tui/06 §6;
    /// `components/image.ts`). The `@`-mention of an image file and clipboard-image paste both land here.
    pub fn attach_image(&mut self, image: ImageBlock) {
        self.state.pending_images.push(image);
    }

    /// Attach an image file by path (the `@`-mention image source); a no-op (returns `false`) when the
    /// path is not a decodable image, so a stray mention never disrupts the prompt.
    pub fn attach_image_path(&mut self, path: &std::path::Path) -> bool {
        match ImageBlock::from_path(path) {
            Some(block) => {
                self.state.pending_images.push(block);
                true
            }
            None => false,
        }
    }

    /// Insert the temp-file PATH of a pasted clipboard image at the editor cursor as ordinary text —
    /// Pi's literal mechanism (`this.editor.insertTextAtCursor(filePath)`,
    /// interactive-mode.ts:2552). The bare path becomes editable text and, on submit, rides the
    /// outgoing user message AS TEXT (no image content block): the agent loads the raster on demand
    /// via a file-read tool, so a potentially huge image never floods context — Pi's deliberate
    /// context-economy choice, which the former `pending_images` embed here violated. Kept separate
    /// from the clipboard read so the path→editor step is unit-testable without a live system
    /// clipboard (`try_paste_clipboard_image_path` supplies the path in the binary).
    fn insert_clipboard_image_path(&mut self, path: &std::path::Path) {
        self.state.editor.insert_str(&path.to_string_lossy());
    }

    /// Read a system-clipboard image, materialize it to a `cyrup-clipboard-<uuid>.png` temp file, and
    /// insert its PATH as text at the editor cursor (Pi `handleClipboardImagePaste`,
    /// interactive-mode.ts:2537-2557). Returns `true` when an image was found and its path pasted;
    /// `false` when the clipboard holds no image (Pi `clipboard.hasImage()` gate) or on any
    /// clipboard/encode/IO error — so the caller lets Ctrl+V fall through to the editor, preserving
    /// normal text-paste behavior.
    fn try_paste_clipboard_image_path(&mut self) -> bool {
        match read_clipboard_image_to_temp() {
            Some(path) => {
                self.insert_clipboard_image_path(&path);
                true
            }
            None => false,
        }
    }

    /// Clear all attached images (after the prompt is sent, or on `Esc`).
    pub fn clear_images(&mut self) {
        self.state.pending_images.clear();
    }

    /// The images attached to the next prompt (test/inspection access).
    pub fn pending_images(&self) -> &[ImageBlock] {
        &self.state.pending_images
    }

    /// Env-sniff the controlling terminal's capabilities (feature #7; Pi `detectCapabilities`) and
    /// upgrade the portable half-block default to the negotiated image protocol (Kitty/iTerm2), while
    /// caching the resolved [`TerminalCapabilities`] so the OSC-8 hyperlink gate (feature #8) can read
    /// them. Called by the binary at startup; tests keep the half-block default (the inline path still
    /// renders to `TestBackend`).
    pub fn detect_image_support(&mut self) {
        let caps = crate::image::detect_capabilities();
        self.state.capabilities = caps;
        self.state.image_renderer = ImageRenderer::from_capabilities(caps);
    }

    /// Apply a new theme, bumping its generation so caches invalidate (R-10-026). The theme is
    /// re-projected through the app's live [`ColorMode`] (feature #3/#4) so a `/theme` switch or hot
    /// reload on a 256-color terminal keeps indexed colors (`with_color_mode` is idempotent for an
    /// already-projected theme).
    pub fn set_theme(&mut self, theme: UiTheme) {
        let mut theme = theme.with_color_mode(self.state.color_mode);
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// Boot the render theme from a [`ThemeController`] (feature #4): adopt the controller's resolved
    /// color mode and set the projected theme. This is the seam the binary uses to honor
    /// `settings.theme` + the terminal background at startup instead of the hardwired dark boot.
    pub fn apply_theme_controller(&mut self, controller: &ThemeController) {
        self.state.color_mode = controller.color_mode();
        let mut theme = controller.theme();
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// The app's active color mode (test/inspection).
    pub fn color_mode(&self) -> ColorMode {
        self.state.color_mode
    }

    /// Re-bind the UI to a freshly-installed runtime session (arch-11 §3.4 replacement; Pi's
    /// interactive session-swap). Called by the run loop on a generation bump (a `/new`/`/resume`/
    /// `/fork`/`/reload`/`/import` op or a runtime-side `SessionReplaced`): the run loop has already
    /// dropped the stale subscription and re-subscribed the new session's `AgentSessionEvent` stream;
    /// here we reset the per-session UI state (the transcript, the streaming/indicator status, any
    /// open selector/overlay) for the new session and surface the swap status line. Committed
    /// scrollback already lives in the terminal's native history (`insert_before`) and is preserved.
    pub fn rebind_session(&mut self) {
        self.state.transcript = TranscriptView::new();
        self.state.selector = None;
        self.state.overlays.clear();
        self.state.status.set_streaming(false);
        self.state.status.set_queued(0);
        self.state.indicator.idle();
        // The new session starts idle, so drop the prior turn's grow-only height floor; the next
        // `draw` collapses the viewport to the compact idle region (void-fix).
        self.live_floor = 0;
        let msg = self.state.pending_swap_status.take().unwrap_or_else(|| "session replaced".into());
        self.state.transcript.push_status(msg);
    }

    /// Seed the transcript from a session's persisted conversation — Pi's `renderInitialMessages()`
    /// → `renderSessionEntries(buildContextEntries(), {updateFooter, populateHistory})`
    /// (interactive-mode.ts:3548-3562) and the `rebuildChatFromMessages()` used after a compaction
    /// or a tree/fork navigation (`:3599-3601`, `:1737-1742`).
    ///
    /// Without this a `/resume`, `/fork`, `/import`, `--resume` or `--continue` shows an EMPTY view
    /// even though the session file holds the whole conversation, because
    /// [`rebind_session`](Self::rebind_session) starts the new session from a fresh
    /// [`TranscriptView`].
    ///
    /// **Feed it [`AgentSession::raw_context_messages`], never `AgentSession::messages()`.** The
    /// latter is the LLM boundary (`convertToLlm`, `messages.ts:148-195`): it has already rendered a
    /// compaction summary, a branch summary, an extension `custom` message and a `!` bash execution
    /// down to `user` messages carrying wrapper prose ("The conversation history before this point
    /// was compacted into the following summary: …"), which would replay as the *user* having typed
    /// that text — and would seed it into the editor's Up-arrow history. Pi feeds the RAW projection
    /// for exactly this reason: `renderSessionEntries` maps entries through
    /// `sessionEntryToContextMessages` (interactive-mode.ts:3506-3516) whose roles are still
    /// `compactionSummary`/`branchSummary`/`custom`/`bashExecution`, and `addMessageToChat`
    /// (`:3308-3350`) routes each to its own component.
    ///
    /// The port follows Pi's `renderSessionItems` walk (`:3415-3497`) + `addMessageToChat`
    /// (`:3308-3413`):
    /// * `user` → the user block (a `<skill …>` submission still splits into its `[skill]`
    ///   invocation + the trailing message, via [`TranscriptView::push_user`]) and, like Pi's
    ///   `populateHistory`, the prompt is pushed into the editor's Up-arrow history;
    /// * `assistant` → the reasoning section, the answer markdown, a live tool block per `toolCall`
    ///   content, and the not-finished-cleanly notice ([`stop_reason_notice`]);
    /// * `toolResult` → attached to the matching open tool block by tool name, then the finished
    ///   leading run is committed so tools land between the assistant turns that bracket them
    ///   rather than all at the end;
    /// * `bashExecution` → a committed bash block (`BashExecutionComponent`, `:3310-3322`),
    ///   dim-bordered for a `!!` (`excludeFromContext`) run;
    /// * `custom` → the labeled extension block, **only when `display`** (`:3323-3336`);
    /// * `compactionSummary` / `branchSummary` → their own summary blocks (`:3337-3350`).
    ///
    /// **ADR-0001 divergence, deliberate**: Pi calls `chatContainer.clear()` before replaying, which
    /// wipes the previous session off the screen. cyrup's committed entries live in the terminal's
    /// native scrollback (`insert_before`) and cannot be erased, so after a mid-session `/resume`
    /// the previous conversation stays visible ABOVE the replayed one. The replay itself needs no
    /// re-render: it starts from an empty transcript and flushes forward normally.
    ///
    /// **Known gap**: Pi resolves an extension's registered message renderer for a replayed `custom`
    /// message (`getMessageRenderer(message.customType)`, `:3326`). cyrup's renderer lookup is async
    /// with a timeout (see `render_with_extensions`), and this walk is synchronous, so a replayed
    /// custom message draws with the built-in `[type] body` framing. The LIVE path
    /// ([`AgentSessionEvent::MessageEnd`]) still honors the renderer.
    pub fn replay_session(&mut self, messages: &[cyrup_session_svc::agent_message::AgentMessage]) {
        use cyrup_core::{Content, Message};
        use cyrup_session_svc::agent_message::AgentMessage;
        use serde_json::Value;
        for message in messages {
            match message {
                AgentMessage::Core(Message::User { content, .. }) => {
                    let text = content_text(content);
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.state.transcript.push_user(text.clone());
                    // Pi `populateHistory` (interactive-mode.ts:3387): replayed prompts are
                    // recallable with Up, so a resumed session can re-run its own last message.
                    self.state.editor.push_history(&text);
                }
                AgentMessage::Core(Message::Assistant(m)) => {
                    let thinking = thinking_text(&m.content);
                    if !thinking.is_empty() {
                        self.state.transcript.commit_thinking(Some(thinking));
                    }
                    let text = content_text(&m.content);
                    if !text.trim().is_empty() {
                        self.state.transcript.commit_assistant(Some(text));
                    }
                    for call in m.content.iter().filter_map(|c| match c {
                        Content::ToolCall(call) => Some(call),
                        _ => None,
                    }) {
                        // Pi files each replayed call component under `content.id`
                        // (`renderedPendingTools.set(content.id, component)`,
                        // interactive-mode.ts:3473) so the `toolResult` below resolves to the exact
                        // call that produced it — two `read`s in one turn are indistinguishable by
                        // name.
                        self.state.transcript.push_tool_start_rendered(
                            call.name.clone(),
                            Some(call.id.as_str().to_string()),
                            Value::Object(call.arguments.clone()),
                            None,
                        );
                    }
                    if let Some(notice) = stop_reason_notice(m) {
                        self.state.transcript.push_error(notice);
                    }
                }
                AgentMessage::Core(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    is_error,
                    details,
                    ..
                }) => {
                    // The shape every per-tool `renderResult` reads (`{content, details}`).
                    let mut result = serde_json::Map::new();
                    result.insert(
                        "content".to_string(),
                        serde_json::to_value(content).unwrap_or(Value::Null),
                    );
                    if let Some(d) = details {
                        result.insert("details".to_string(), d.clone());
                    }
                    // `renderedPendingTools.get(message.toolCallId)` (`:3483`) — an exact id lookup,
                    // never a name scan.
                    self.state.transcript.push_tool_end_rendered(
                        tool_name.clone(),
                        Some(tool_call_id.as_str()),
                        *is_error,
                        Some(Value::Object(result)),
                        None,
                    );
                    // Keep call order in scrollback: commit the finished leading run now instead of
                    // deferring every tool of the whole replay to the end.
                    self.state.transcript.commit_finished_leading_tools();
                }
                AgentMessage::BashExecution(b) => {
                    self.state.transcript.push_bash_execution(
                        b.command.clone(),
                        b.exclude_from_context.unwrap_or(false),
                        &b.output,
                        b.exit_code.and_then(|c| i32::try_from(c).ok()),
                        b.cancelled,
                    );
                }
                AgentMessage::Custom(c) => {
                    // Pi renders a custom message only when it opted into display
                    // (`if (message.display)`, interactive-mode.ts:3324).
                    if c.display {
                        self.state
                            .transcript
                            .push_custom_message(c.custom_type.clone(), custom_message_text(&c.content));
                    }
                }
                AgentMessage::BranchSummary(b) => {
                    self.state.transcript.push_branch_summary(b.summary.clone());
                }
                AgentMessage::CompactionSummary(c) => {
                    self.state
                        .transcript
                        .push_compaction_summary(c.tokens_before, c.summary.clone());
                }
            }
        }
        // A tool call whose result never persisted (an interrupted turn) still commits, as-is.
        self.state.transcript.commit_tools();
    }

    /// Emit the startup loaded-resources / diagnostics panel (Pi `showLoadedResources`,
    /// interactive-mode.ts:1480-1690, called with `{force: false, showDiagnosticsWhenQuiet: true}`
    /// at `:1769`).
    ///
    /// TUI-006: without this, extension load failures, shadowed skills and missing prompt paths were
    /// entirely invisible in cyrup — the data existed (`AgentSessionServices::startup_diagnostics`)
    /// but nothing rendered it. Push it before the first draw so it lands at the top of scrollback,
    /// ahead of the conversation.
    pub fn push_loaded_resources(&mut self, report: &crate::startup::StartupReport) {
        self.state.transcript.push_loaded_resources(crate::startup::build_startup_lines(report));
    }

    /// Put already-queued steering/follow-up text back into the editor — the buffer half of Pi's
    /// `restoreQueuedMessagesToEditor` (interactive-mode.ts:4064-4083). `queued` is
    /// `[...steering, ...followUp]` **already drained** from the session (Pi's `clearAllQueues()`
    /// at `:4065`, here [`AgentSession::drain_queue`]); this half is pure, so the run loop owns the
    /// async drain and the abort and the App owns what the user sees.
    ///
    /// The queued messages join with a blank line and are PREPENDED to whatever is already typed,
    /// with empty parts dropped (`:4074-4077` — `[queuedText, currentText].filter(t => t.trim())`).
    /// An empty queue leaves the editor untouched and returns `0`, which is how
    /// [`AppAction::Dequeue`] decides between Pi's two `handleDequeue` statuses (`:3834-3841`).
    /// The Esc path (`{abort: true}`) shows no status at all — Pi's escape branch never calls
    /// `showStatus`.
    pub fn restore_queued_to_editor(&mut self, queued: &[String]) -> usize {
        if queued.is_empty() {
            return 0;
        }
        let queued_text = queued.join("\n\n");
        let current = self.state.editor.text();
        let combined = [queued_text, current]
            .into_iter()
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.state.editor.set_text(&combined);
        queued.len()
    }

    /// Render one frame: first flush newly-committed entries to native scrollback (R-ARCH-TUI-003),
    /// then draw the active region into the inline viewport (pure: `state -> frame`).
    pub fn draw(&mut self) -> Result<(), TuiError>
    where
        B: RebuildBackend,
    {
        // Content-size the inline viewport to the live region (active turn + band + slot + footer),
        // recomputed every frame as content grows/shrinks (ADR-0001 #1, audit #1). The viewport is
        // rebuilt only when its height actually changes so steady-state frames keep their cell-diff.
        // Resize **before** flushing so the committed `insert_before` lines scroll above the
        // correctly-anchored viewport (the active turn's height is unaffected by the flush).
        let size = self.terminal.backend().size().ok();
        let term_h = size.map(|s| s.height).unwrap_or(self.viewport_height).max(1);
        let term_w = size.map(|s| s.width).unwrap_or(80);
        let raw = live_region_height(&self.state, term_w, term_h);
        // Grow-only hysteresis GATED on the turn being active. `status.streaming` is set on
        // `AgentStart` and cleared on `AgentEnd`, so it spans the WHOLE multi-step turn including the
        // gaps between tools (it is NOT `transcript.has_active()`, which flickers false between tools
        // and would re-trigger per-tool reconstruction); `has_bash()` covers a live `!`/`!!` run.
        // While active, the viewport pins at its high-water (capped to the terminal height so a
        // resize-shrink still reduces it) and stops tracking per-tool content churn — so
        // `resize_viewport`/`reanchor_inline` fire only on genuine geometry changes, killing the
        // per-tool FLICKER. Idle: drop the floor and size to the live content so the region collapses
        // to the compact editor/footer (void-fix).
        let turn_active = self.state.status.streaming || self.state.transcript.has_bash();
        let desired = if turn_active {
            self.live_floor = self.live_floor.max(raw).min(term_h);
            self.live_floor
        } else {
            self.live_floor = 0;
            raw
        };
        if desired != self.viewport_height {
            self.resize_viewport(desired)?;
            self.viewport_height = desired;
        }
        self.flush_committed()?;
        let App { terminal, state, .. } = self;
        terminal
            .draw(|frame| render(frame, state))
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Rebuild the terminal with a new inline-viewport `height` over a fresh handle to the same
    /// backend (ratatui's inline height is immutable after construction; audit #1). The cursor anchor
    /// is preserved by [`RebuildBackend::rebuild`], so the re-placed viewport stays where it was.
    fn resize_viewport(&mut self, height: u16) -> Result<(), TuiError>
    where
        B: RebuildBackend,
    {
        // Erase the CURRENT inline region and re-anchor the cursor BEFORE reconstructing at the new
        // height, so the reservation's `append_lines` scrolls BLANKS rather than the prior frame's
        // chrome. On a real terminal this is the whole difference between a clean regrow and the
        // hint-bar/editor-rule/footer STACKING the audit hit; a no-op on fresh-grid backends
        // (`TestBackend`), which start each `rebuild` from a blank buffer and can never stack.
        let size = self.terminal.backend().size().ok();
        let term_h = size.map(|s| s.height).unwrap_or(height).max(1);
        let old_h = self.viewport_height;
        self.terminal.backend_mut().reanchor_inline(term_h, old_h, height);

        let backend = self.terminal.backend().rebuild();
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        self.terminal = terminal;
        Ok(())
    }

    /// Move every newly-committed transcript entry into native scrollback via `Terminal::insert_before`
    /// **exactly once** (R-ARCH-TUI-003 / R-10-002), recording the same lines in the test-visible
    /// `scrollback` accumulator. After this the inline viewport only renders the active streaming turn,
    /// the editor, and the status line. A no-op when nothing was committed since the last flush.
    fn flush_committed(&mut self) -> Result<(), TuiError> {
        let committed = self.state.transcript.drain_committed();
        if committed.is_empty() {
            return Ok(());
        }
        // Content width for markdown wrapping: the live terminal width (R-ARCH-TUI-005), fallback 80.
        let width = self.terminal.backend().size().map(|s| s.width as usize).unwrap_or(80);
        let output_pad = self.state.transcript.output_pad();
        // Committed tool-result images keep rendering — a half-block raster is ordinary cells, so it
        // survives `insert_before` into native scrollback (see `ImageBlock::halfblock_lines`).
        let images = crate::transcript::ImageOpts {
            show: self.state.transcript.show_images(),
            width_cells: self.state.transcript.image_width_cells(),
        };
        let lines: Vec<Line<'static>> = committed
            .iter()
            .flat_map(|e| entry_lines(e, &self.state.theme, width, output_pad, images))
            .collect();
        self.state.scrollback.extend(lines.iter().cloned());
        let style = self.state.theme.base_style();
        // Size the scrollback slot to the WRAPPED display-row count (not `lines.len()`) and render
        // WITH `.wrap()`: `entry_lines` emits one un-wrapped `Line` per prose paragraph, so a long
        // committed answer must wrap to width and reserve its wrapped height — otherwise
        // `insert_before` clips it to a single row and the full text is lost from native scrollback
        // (the PROSE-WRAP truncation; R-ARCH-TUI-003/-005, spec/tui/01 §3 overflow).
        let height = crate::transcript::wrapped_height(&lines, width).min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(lines).style(style).wrap(Wrap { trim: false }).render(buf.area, buf);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Map one input event to an [`AppAction`], mutating editor/transcript as needed. Global keys
    /// (resolved via the configurable [`Keymap`], R-10-018) take precedence over editing keys.
    pub fn handle_input(&mut self, ev: &InputEvent) -> AppAction {
        match ev {
            InputEvent::Key(key) => {
                if matches!(key.kind, KeyEventKind::Release) {
                    return AppAction::None;
                }
                // A floating overlay (hotkeys/help popup) captures input first (spec/tui/05 §2
                // routing step 2): the topmost overlay handles the key; `Close` pops it, an unhandled
                // key is swallowed so it never leaks to the editor/agent beneath the modal.
                if !self.state.overlays.is_empty() {
                    return self.handle_overlay_key(key);
                }
                // A focused selector captures input first (spec/tui/05 §2 routing step 2): its
                // navigation/confirm/cancel keys are handled before the global keymap, so `Esc`/`Ctrl+C`
                // dismiss the selector rather than interrupting the agent. Unbound keys fall through.
                if self.state.selector.is_some() {
                    return self.handle_selector_key(key);
                }
                // Routing chain: overlay > completion > editor > app (spec/tui/07 §2). A global key is
                // resolved here, but two context guards defer it to the editor so the chain holds
                // (audit #4 — the previous unconditional global resolution made Ctrl+D quit and Esc
                // abort an open popup):
                //   • Esc with the completion popup open dismisses the popup, never aborts the run
                //     (spec/tui/04 §5, spec/tui/07 §7).
                //   • Ctrl+D on a non-empty buffer is forward-delete; it only exits on empty
                //     (spec/tui/03 §6, spec/tui/07 §3.3).
                if let Some(action) = self.state.keymap.action_for(key) {
                    // `app.clipboard.pasteImage` (Ctrl+V): read a system-clipboard image and insert its
                    // temp-file PATH as text at the editor cursor (Pi `handleClipboardImagePaste` →
                    // `insertTextAtCursor(filePath)`, interactive-mode.ts:2537-2557). Gated on an image
                    // actually being present (Pi `clipboard.hasImage()`): when the clipboard holds no
                    // image the key is NOT swallowed — it falls through to the editor below so normal
                    // Ctrl+V behavior is preserved (do not break text paste).
                    if action == Action::ClipboardPasteImage {
                        if self.try_paste_clipboard_image_path() {
                            return AppAction::Redraw;
                        }
                        // No image on the clipboard: fall through to the editor (text) handling below.
                    } else {
                        let defer_to_editor = match action {
                            Action::Interrupt => self.state.editor.autocomplete_open(),
                            Action::Quit => !self.state.editor.is_empty(),
                            _ => false,
                        };
                        if !defer_to_editor {
                            return self.apply_action(action);
                        }
                    }
                }
                // An extension-registered keyboard shortcut (R-08-017; Pi `registerShortcut`) fires at
                // the global-keymap tier — after the built-in bindings (so an extension can't shadow
                // `Ctrl+D`/`Esc`) but before the editor (so the key never leaks in as text). The run
                // loop dispatches the matched key-id to the session's extension host.
                if let Some((_, id)) =
                    self.state.extension_shortcuts.iter().find(|(k, _)| k.matches(key))
                {
                    return AppAction::ExtensionShortcut(id.clone());
                }
                match self.state.editor.handle_key(key) {
                    EditorOutcome::Submit(text) => self.dispatch_submission(&text),
                    EditorOutcome::Edited => AppAction::Redraw,
                    EditorOutcome::Ignored => AppAction::None,
                }
            }
            InputEvent::Paste(s) => {
                // A selector owns the slot: pure-list selectors ignore pastes (no embedded Input yet).
                if self.state.selector.is_some() {
                    return AppAction::None;
                }
                // Route bracketed paste through `handle_paste` so large pastes collapse to an atomic
                // `[paste #N …]` marker (spec/tui/03 §5.5); small pastes insert verbatim.
                self.state.editor.handle_paste(s);
                AppAction::Redraw
            }
            InputEvent::Resize(_, _) => AppAction::Redraw,
            InputEvent::FocusGained => {
                self.state.editor.set_focused(true);
                AppAction::Redraw
            }
            InputEvent::FocusLost => {
                self.state.editor.set_focused(false);
                AppAction::Redraw
            }
        }
    }

    /// Classify a submitted line via the [`CommandRegistry`] and route it (spec/tui/04 §2.3).
    ///
    /// A plain prompt is echoed into the transcript and returned as [`AppAction::Submit`] for the run
    /// loop to deliver to the runtime. A recognized slash command or a `!`/`!!` bash invocation is
    /// surfaced as a status line for now — opening the bound overlay / executing bash is wired as the
    /// selector + bash-execution subsystems land (tracked on the residual ledger). This keeps the
    /// editor → dispatch path real and faithful (commands never reach the agent as literal text).
    fn dispatch_submission(&mut self, text: &str) -> AppAction {
        let dispatch = self.state.commands.dispatch(text);
        // The startup hint bar is a first-run affordance; any real submission dismisses it
        // (Pi drops `compactInstructions` once the conversation begins).
        if !matches!(dispatch, Dispatch::Empty) {
            self.state.show_startup_hints = false;
        }
        match dispatch {
            Dispatch::Empty => AppAction::Redraw,
            Dispatch::Prompt(prompt) => {
                self.state.transcript.push_user(prompt.clone());
                AppAction::Submit(prompt)
            }
            Dispatch::Command { name, arg } => self.run_command(&name, arg),
            Dispatch::Bash { command, excluded } => {
                // Open the live bash block (`bash-execution.ts`) and hand the spawn to the run loop.
                let cancel = self.state.keymap.key_label(Action::Interrupt);
                let expand = self.state.keymap.key_label(Action::ToolsExpand);
                self.state.transcript.start_bash(command.clone(), excluded, cancel, expand);
                AppAction::RunBash { command, excluded }
            }
        }
    }

    /// Route a recognized slash command (`setupEditorSubmitHandler`, interactive-mode.ts:2554-2734).
    /// In-crate effects (info blocks, quit, easter eggs) are applied here directly and return
    /// [`AppAction::Redraw`]; session/data-bound effects return [`AppAction::Command`] for the run
    /// loop to execute against the [`AgentSession`]. Note `/theme`, `/think`, `/show-images` are NOT
    /// commands — Pi has no such builtins, so they fall through [`CommandRegistry::dispatch`] to
    /// [`Dispatch::Prompt`] and reach the agent as literal text (theme is reached via `/settings` →
    /// Theme, thinking via Shift+Tab; see [`Action::ThinkingCycle`]).
    fn run_command(&mut self, name: &str, arg: Option<String>) -> AppAction {
        use AppCommand as C;
        let cmd = |c| AppAction::Command(c);
        match name {
            // --- data-bound selectors (run loop sources rows) ---
            // `/model [text]` threads its argument (`handleModelCommand(searchTerm?)`,
            // interactive-mode.ts:4175): exact match → set directly; partial → pre-filtered picker.
            "model" => cmd(C::ModelCommand(arg)),
            "settings" => cmd(C::OpenSelector(SelectorKind::Settings)),
            "scoped-models" => cmd(C::OpenSelector(SelectorKind::ScopedModels)),
            "tree" => cmd(C::OpenSelector(SelectorKind::Tree)),
            "resume" => cmd(C::OpenSelector(SelectorKind::Session)),
            "trust" => cmd(C::OpenSelector(SelectorKind::Trust)),
            "fork" => cmd(C::OpenSelector(SelectorKind::UserMessage)),
            "login" => cmd(C::OpenSelector(SelectorKind::Login)),
            "logout" => cmd(C::OpenSelector(SelectorKind::Logout)),
            // --- session lifecycle / IO (run loop) ---
            "new" => cmd(C::NewSession),
            "compact" => cmd(C::Compact(arg)),
            "clone" => cmd(C::Clone),
            "reload" => cmd(C::Reload),
            "export" => cmd(C::Export(arg)),
            "import" => cmd(C::Import(arg)),
            "share" => cmd(C::Share),
            "copy" => cmd(C::Copy),
            "name" => match arg {
                Some(n) => cmd(C::SetName(n)),
                None => {
                    self.state.transcript.push_status("usage: /name <session name>");
                    AppAction::Redraw
                }
            },
            "session" => cmd(C::SessionInfo),
            // --- in-crate info blocks ---
            "hotkeys" => {
                // A floating, dismissable overlay (spec/tui/05 §2), not a scrollback block.
                self.open_hotkeys_overlay();
                AppAction::Redraw
            }
            "changelog" => {
                self.state.transcript.push_block("What's New", "No changelog entries found.");
                AppAction::Redraw
            }
            "debug" => {
                let body = self.debug_markdown();
                self.state.transcript.push_block("Debug", body);
                AppAction::Redraw
            }
            "quit" => {
                self.state.should_quit = true;
                AppAction::Quit
            }
            // `/arminsayshi` (`armin.ts` `ArminComponent`): the 31×36 XBM bitmap rendered with
            // half-block glyphs (the random CRT/glitch animation effects are non-deterministic chrome
            // and omitted; the art itself is a real rich render, not a status line).
            "arminsayshi" => {
                self.state.transcript.push_block("Armin says hi!", armin_art());
                AppAction::Redraw
            }
            // `/dementedelves` (`daxnuts.ts`): a themed banner block (the model-triggered animation is
            // chrome; the announcement is a real rich block).
            "dementedelves" => {
                self.state
                    .transcript
                    .push_block("Demented Elves", "🧝 The demented elves have entered the chat.");
                AppAction::Redraw
            }
            // Any other unhandled recognized name: a status line.
            other => {
                self.state.transcript.push_status(format!("command: /{other}"));
                AppAction::Redraw
            }
        }
    }

    /// Build the `/debug` info block (`handleDebugCommand`, interactive-mode.ts:5526): terminal size,
    /// active theme + generation, thinking level, and selector/stream state.
    fn debug_markdown(&self) -> String {
        let size = self.terminal.backend().size().ok();
        let (w, h) = size.map(|s| (s.width, s.height)).unwrap_or((0, 0));
        format!(
            "| Field | Value |\n|-------|-------|\n\
             | terminal | {w}×{h} |\n\
             | theme | {} (gen {}) |\n\
             | thinking | {} |\n\
             | show images | {} |\n\
             | streaming | {} |\n",
            self.state.theme.name,
            self.state.theme.generation,
            self.state.thinking_level,
            self.state.show_images,
            self.state.status.streaming,
        )
    }

    /// Resolve a global keymap action (R-10-024 Ctrl+C, R-10-030 abort).
    fn apply_action(&mut self, action: Action) -> AppAction {
        match action {
            Action::Quit => {
                self.state.should_quit = true;
                AppAction::Quit
            }
            Action::Interrupt => {
                // A running `!`/`!!` bash block is cancelled first (the run loop kills the child),
                // mirroring Pi's `tui.select.cancel` on the bash component.
                if self.state.transcript.bash_running() {
                    self.state.transcript.bash_complete(None, true);
                    self.state.transcript.commit_bash();
                }
                // Pi branches on `this.session.isStreaming` FIRST (interactive-mode.ts:2636): an Esc
                // that lands mid-turn restores the queued steering/follow-up text to the editor and
                // THEN aborts, so nothing the user typed during the run is lost. Read the flag
                // before the local teardown below clears it.
                let streaming = self.state.status.streaming;
                self.state.transcript.discard_streaming();
                self.state.transcript.commit_tools();
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                if streaming {
                    AppAction::InterruptRestoreQueued
                } else {
                    AppAction::Interrupt
                }
            }
            // `app.clear` (Ctrl+C, Pi `handleCtrlC` interactive-mode.ts:3361-3369): a second Ctrl+C
            // within 500 ms of the previous one EXITS — there is NO emptiness gate (Pi does not require
            // the editor to be empty; that is `Ctrl+D`'s rule, not `Ctrl+C`'s). Otherwise clear the
            // editor buffer and record the press time.
            Action::Clear => {
                let now = std::time::Instant::now();
                let double_tap = self
                    .state
                    .last_sigint
                    .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(500));
                if double_tap {
                    self.state.should_quit = true;
                    AppAction::Quit
                } else {
                    self.state.editor.clear();
                    self.state.last_sigint = Some(now);
                    AppAction::Redraw
                }
            }
            // `app.tools.expand` (Ctrl+O) toggles tool-output expansion in-crate (`tool-execution.ts`
            // expand); the live tools re-render expanded/collapsed on the next frame.
            Action::ToolsExpand => {
                // `Ctrl+O` toggles the live bash block when one is present (`bash-execution.ts`
                // `setExpanded`), else the tool-output expansion.
                if self.state.transcript.has_bash() {
                    self.state.transcript.toggle_bash_expanded();
                } else {
                    self.state.transcript.toggle_tool_expanded();
                }
                AppAction::Redraw
            }
            // `app.suspend` (Ctrl+Z) is surfaced to the run loop, which tears down raw mode, raises
            // SIGTSTP, and re-enters on SIGCONT (the raise lives in an isolated allow-unsafe shim).
            Action::Suspend => AppAction::Suspend,
            // `app.editor.external` (Ctrl+G): surfaced to the run loop, which restores the terminal,
            // launches `$VISUAL`/`$EDITOR` on the buffer, and reloads it.
            Action::ExternalEditor => AppAction::OpenExternalEditor,
            // Page scroll over the **active region** (spec/tui/07): committed history lives in the
            // terminal's native scrollback (paged with the terminal's own scroll, ADR-0001), but the
            // in-flight streaming/tool/bash output can exceed the viewport — `PageUp`/`PageDown` page
            // it without losing the live tail. The page size is one screenful, resolved at render time
            // against the live viewport; a fixed conservative page keeps this input-thread pure.
            Action::PageUp => {
                self.state.transcript.page_up(PAGE_SCROLL_LINES);
                AppAction::Redraw
            }
            Action::PageDown => {
                self.state.transcript.page_down(PAGE_SCROLL_LINES);
                AppAction::Redraw
            }
            // `app.thinking.cycle` (Shift+Tab): advance the reasoning level in place — no picker. The
            // cycle is GATED on the live model supporting thinking and walks the model's OWN supported
            // levels, so it rides an `AppCommand` the run loop resolves against the session (Pi
            // `cycleThinkingLevel` calls `session.cycleThinkingLevel()`, interactive-mode.ts:3606-3614;
            // agent-session.ts:1599). The footer + editor rule re-color off the emitted
            // `ThinkingLevelChanged` event, exactly as in Pi's event handler (interactive-mode.ts:2804).
            Action::ThinkingCycle => AppAction::Command(AppCommand::CycleThinking),
            // `app.model.cycleForward` / `cycleBackward` (Ctrl+P / Shift+Ctrl+P): the model swap needs
            // the live catalog + `set_model` at the session layer, so it rides an `AppCommand` the run
            // loop applies (Pi `cycleModel`, interactive-mode.ts:3617-3632).
            Action::ModelCycleForward => {
                AppAction::Command(AppCommand::CycleModel(CycleDirection::Forward))
            }
            Action::ModelCycleBackward => {
                AppAction::Command(AppCommand::CycleModel(CycleDirection::Backward))
            }
            // `app.message.followUp` (Alt+Enter): queue the editor text as a follow-up delivered after
            // the turn goes idle (Pi `handleFollowUp`, interactive-mode.ts:3554-3585). Empty input is a
            // no-op (Pi's `if (!text) return`); the streaming-vs-idle decision + delivery is async, so
            // it rides an `AppAction` the run loop resolves against the live session.
            Action::FollowUp => {
                let text = self.state.editor.text();
                if text.trim().is_empty() {
                    AppAction::Redraw
                } else {
                    AppAction::FollowUp(text)
                }
            }
            // `app.message.dequeue` (Alt+Up): restore queued messages to the editor. The queue read +
            // clear are on the live session, so it rides an `AppAction` the run loop resolves (Pi
            // `handleDequeue`, interactive-mode.ts:3587-3594).
            Action::Dequeue => AppAction::Dequeue,
            // `app.clipboard.pasteImage` (Ctrl+V) is resolved earlier in `handle_input` — it must be
            // able to fall through to the editor when the clipboard holds no image, which this arm
            // cannot do — so this arm is normally unreachable. It exists only to keep the match
            // exhaustive; it pastes the path best-effort and redraws (no panic, per the no-panic policy).
            Action::ClipboardPasteImage => {
                self.try_paste_clipboard_image_path();
                AppAction::Redraw
            }
        }
    }

    /// Route one key to the topmost floating overlay (spec/tui/05 §2 step 2). `Close` pops it; any
    /// other outcome stays open and redraws. A no-op when the stack is empty.
    fn handle_overlay_key(&mut self, key: &event::KeyEvent) -> AppAction {
        let Some(top) = self.state.overlays.last_mut() else { return AppAction::None };
        match top.handle(key) {
            OverlayOutcome::Close => {
                self.state.overlays.pop();
                AppAction::Redraw
            }
            OverlayOutcome::Redraw | OverlayOutcome::Ignored => AppAction::Redraw,
        }
    }

    /// Push the hotkeys/help popup onto the overlay stack (`/hotkeys` → `handleHotkeysCommand`,
    /// interactive-mode.ts:5396-5470). Rows are derived from the **live** keymaps so rebinds reflect.
    pub fn open_hotkeys_overlay(&mut self) {
        let rows = self.hotkey_rows();
        self.state.overlays.push(Box::new(HotkeysOverlay::new("Keyboard Shortcuts", rows)));
    }

    /// Whether a floating overlay is currently open (test/inspection access).
    pub fn overlay_open(&self) -> bool {
        !self.state.overlays.is_empty()
    }

    /// Build the hotkeys popup rows from the live editor + global keymaps (the structured form of
    /// [`hotkeys_markdown`](Self::hotkeys_markdown); same bindings, rebind-aware).
    fn hotkey_rows(&self) -> Vec<HotkeyRow> {
        let ek = self.state.editor.keymap_ref();
        let km = &self.state.keymap;
        let e = |a: EditorAction| ek.key_label(a).unwrap_or_else(|| "—".to_string());
        let g = |a: Action| km.key_label(a).unwrap_or_else(|| "—".to_string());
        let entry = |keys: String, desc: &str| HotkeyRow::Entry { keys, desc: desc.to_string() };
        vec![
            HotkeyRow::Section("Navigation".to_string()),
            entry(
                format!(
                    "{}/{}/{}/{}",
                    e(EditorAction::CursorUp),
                    e(EditorAction::CursorDown),
                    e(EditorAction::CursorLeft),
                    e(EditorAction::CursorRight)
                ),
                "Move cursor / browse history",
            ),
            entry(
                format!("{}/{}", e(EditorAction::CursorWordLeft), e(EditorAction::CursorWordRight)),
                "Move by word",
            ),
            entry(e(EditorAction::CursorLineStart), "Start of line"),
            entry(e(EditorAction::CursorLineEnd), "End of line"),
            entry(e(EditorAction::JumpForward), "Jump forward to character"),
            entry(e(EditorAction::JumpBackward), "Jump backward to character"),
            entry(format!("{}/{}", g(Action::PageUp), g(Action::PageDown)), "Scroll by page"),
            HotkeyRow::Section("Editing".to_string()),
            entry(e(EditorAction::Submit), "Send message"),
            entry(e(EditorAction::NewLine), "New line"),
            entry(e(EditorAction::DeleteWordBackward), "Delete word backwards"),
            entry(e(EditorAction::DeleteWordForward), "Delete word forwards"),
            entry(e(EditorAction::DeleteToLineStart), "Delete to start of line"),
            entry(e(EditorAction::DeleteToLineEnd), "Delete to end of line"),
            entry(e(EditorAction::Yank), "Paste most-recently-deleted text"),
            entry(e(EditorAction::YankPop), "Cycle deleted text after pasting"),
            entry(e(EditorAction::Undo), "Undo"),
            HotkeyRow::Section("Other".to_string()),
            entry(e(EditorAction::Tab), "Path completion / accept autocomplete"),
            entry(g(Action::Interrupt), "Cancel autocomplete / abort streaming"),
            entry(g(Action::Clear), "Clear editor (first) / exit (second)"),
            entry(g(Action::Quit), "Quit"),
            entry(g(Action::Suspend), "Suspend to background"),
            entry(g(Action::ToolsExpand), "Toggle tool output expansion"),
            entry(g(Action::ExternalEditor), "Open buffer in external editor"),
            entry(g(Action::ThinkingCycle), "Cycle thinking level"),
            entry(
                format!("{}/{}", g(Action::ModelCycleForward), g(Action::ModelCycleBackward)),
                "Cycle model forward / backward",
            ),
            entry(g(Action::FollowUp), "Queue message as a follow-up"),
            entry(g(Action::Dequeue), "Restore queued messages to editor"),
            entry(g(Action::ClipboardPasteImage), "Paste image from clipboard"),
            entry("/".to_string(), "Slash commands"),
            entry("!".to_string(), "Run bash command"),
            entry("!!".to_string(), "Run bash command (excluded from context)"),
        ]
    }

    /// Open an editor-swap selector (spec/tui/05 §1.1 `showSelector`): snapshot the editor text, build
    /// the selector for `kind`, and put it in the input slot. The theme picker also stashes the live
    /// theme so a cancel can restore it. Idempotent-ish: opening replaces any already-open selector.
    pub fn open_selector(&mut self, kind: SelectorKind) {
        let saved_editor = self.state.editor.text();
        let (inner, restore_theme): (Box<dyn Selector>, Option<UiTheme>) = match kind {
            SelectorKind::Thinking => {
                (Box::new(ListSelector::thinking(&self.state.thinking_level)), None)
            }
            SelectorKind::ShowImages => {
                (Box::new(ListSelector::show_images(self.state.show_images)), None)
            }
            SelectorKind::Theme => (
                Box::new(ListSelector::theme(&self.state.theme.name)),
                Some(self.state.theme.clone()),
            ),
            // Data-bound selectors must be opened via `open_data_selector` (they need L5 rows);
            // opening one with no data yields an empty-state list rather than a panic.
            other => (Box::new(ListSelector::data(other, Vec::new(), 0)), None),
        };
        self.state.selector = Some(ActiveSelector { kind, inner, saved_editor, restore_theme });
    }

    /// Open a **data-bound** selector (`/model`, `/resume`, `/tree`, …) over rows the run loop sourced
    /// from session-svc / resources (spec/tui/05 §6, §8 late-data population). `rows` are
    /// `(value, label, description)`; `selected` preselects a row. Confirming hands the chosen `value`
    /// back to the run loop as [`AppCommand::ConfirmSelection`].
    pub fn open_data_selector(
        &mut self,
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) {
        let saved_editor = self.state.editor.text();
        let inner: Box<dyn Selector> = Box::new(ListSelector::data(kind, rows, selected));
        self.state.selector =
            Some(ActiveSelector { kind, inner, saved_editor, restore_theme: None });
    }

    /// Open the bespoke scoped-models checkbox+reorder selector (`scoped-models-selector.ts`,
    /// spec/tui/05 §6) over the full `catalog` `(id, label, provider, desc)` with the current scope
    /// (`None` = all enabled). Confirming (Ctrl+S) yields an [`AppCommand::ConfirmSelection`] the run
    /// loop applies via `set_scoped_models`.
    pub fn open_checkbox_selector(
        &mut self,
        catalog: Vec<(String, String, String, Option<String>)>,
        enabled: Option<Vec<String>>,
    ) {
        let saved_editor = self.state.editor.text();
        let inner: Box<dyn Selector> =
            Box::new(CheckboxSelector::scoped_models(catalog, enabled));
        self.state.selector = Some(ActiveSelector {
            kind: SelectorKind::ScopedModels,
            inner,
            saved_editor,
            restore_theme: None,
        });
    }

    /// Open the `/model` selector (feature #1): the full [`ModelSelector`] with fuzzy search, the
    /// `all | scoped` scope toggle, `[provider]` badges, and a `✓` on the active model — over the live
    /// model catalog `(id, name, provider, current, scoped)`. Replaces the bare titled list the audit
    /// flagged. Snapshots the editor like every editor-swap selector. When `search` is `Some`, the
    /// picker opens **pre-filtered** to it (Pi `showModelSelector(initialSearchInput)`,
    /// interactive-mode.ts:4307,4333).
    pub fn open_model_selector(&mut self, models: Vec<ModelEntry>, search: Option<String>) {
        let saved_editor = self.state.editor.text();
        let mut selector = ModelSelector::new(models);
        if let Some(term) = search {
            selector.set_search(term);
        }
        let inner: Box<dyn Selector> = Box::new(selector);
        self.state.selector = Some(ActiveSelector {
            kind: SelectorKind::Model,
            inner,
            saved_editor,
            restore_theme: None,
        });
    }

    /// Handle `/model [text]` (Pi `handleModelCommand`, interactive-mode.ts:4175-4196): with no term the
    /// unfiltered picker opens; with a term, an EXACT catalog match sets the model directly (no picker,
    /// `findExactModelReferenceMatch` → `session.setModel`), while a partial opens the picker
    /// pre-filtered to it. The catalog is the live available multi-provider catalog the picker itself
    /// sources (`model_entries`).
    async fn handle_model_command(&mut self, session: &Arc<AgentSession>, search: Option<String>) {
        let models = model_entries(session);
        if models.is_empty() {
            self.state.transcript.push_status("no models available (configure providers)");
            return;
        }
        if let Some(term) = search.as_deref()
            && let Some(model) = crate::model_selector::find_exact_model_reference_match(&models, term)
        {
            // Exact match → set the fully-qualified `provider/id` directly (mirrors the confirm path),
            // no picker (`handleModelCommand` early-returns after `setModel`).
            let id = format!("{}/{}", model.provider, model.id);
            match session.set_model(&id).await {
                Ok(_) => self.state.transcript.push_status(format!("model → {id}")),
                Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
            }
            return;
        }
        // No term, or a partial with no exact match → the picker, pre-filtered to the term if any.
        self.open_model_selector(models, search);
    }

    /// Open an arbitrary boxed [`Selector`] in the input slot under `kind` (the seam for the bespoke
    /// non-list selectors — `/tree`'s [`TreeSelector`] — that are not a plain [`ListSelector`] yet need
    /// the same editor-swap lifecycle as the data selectors). Snapshots the editor like the others.
    pub fn open_boxed_selector(&mut self, kind: SelectorKind, inner: Box<dyn Selector>) {
        let saved_editor = self.state.editor.text();
        self.state.selector =
            Some(ActiveSelector { kind, inner, saved_editor, restore_theme: None });
    }

    /// The kind of the currently-open selector, if any (test/inspection access).
    pub fn active_selector_kind(&self) -> Option<SelectorKind> {
        self.state.selector.as_ref().map(|s| s.kind)
    }

    /// Render a loaded extension's `ui.{confirm,select,input}` dialog request in the input slot (L4
    /// review §2.1; `ui.editor` is handled synchronously by the caller, never reaching here — see
    /// [`App::run`]'s `ui_rx` arm). Mirrors Pi's `createExtensionUIContext`
    /// (`interactive-mode.ts:2060-2111`): `confirm` opens a Yes/No [`ListSelector`] exactly like Pi's
    /// confirm-as-select (`:2172-2179`); `select` opens a [`ListSelector`] over the guest's option
    /// strings; `input` opens a [`TextInputSelector`]. The dialog's `reply` one-shot is stashed on
    /// [`AppState::pending_ui_reply`]; [`App::confirm_selector`] and the selector-cancel arm of
    /// [`App::handle_selector_key`] take + resolve it when the user answers.
    ///
    /// The input slot holds at most one occupant: if a selector (first-party or extension) or a
    /// floating overlay is already open when a guest dialog arrives, it is denied immediately with its
    /// per-kind deny default (there is nowhere to render it) rather than queued or silently dropped —
    /// the guest's `ui_roundtrip` never blocks past this call regardless.
    ///
    /// `pub` (not called outside [`App::run`]'s `ui_rx` arm in production) so `tests/*.rs` can drive
    /// it directly with a synthetic [`UiRequest`] — the crate's established pattern for exercising
    /// run-loop-only logic (mirrors [`Self::open_boxed_selector`]/[`Self::active_selector_kind`]).
    pub fn open_extension_dialog(&mut self, req: UiRequest) {
        if self.state.selector.is_some() || !self.state.overlays.is_empty() {
            self.state.transcript.push_status(format!(
                "extension {:?} dialog: another dialog/selector is already open, denied",
                req.kind
            ));
            let _ = req.reply.send(default_ui_reply(req.kind));
            return;
        }
        let UiRequest { kind, prompt, options, message, placeholder, opts, reply } = req;
        let (selector_kind, base_title, mut inner): (SelectorKind, String, Box<dyn Selector>) = match kind
        {
            UiKind::Confirm => {
                // Pi's EXACT join (`showExtensionConfirm`, `interactive-mode.ts:2177`):
                // `` `${title}\n${message}` `` — a real newline, not an em-dash. The title area
                // now auto-sizes + word-wraps (`ListSelector::desired_height`/`render`,
                // `title_wrapped_height`) so a long title and/or multi-line message both render in
                // full instead of being clipped to one row (L4 review §2.6).
                let title = if message.is_empty() { prompt } else { format!("{prompt}\n{message}") };
                let rows = vec![
                    ("yes".to_string(), "Yes".to_string(), None),
                    ("no".to_string(), "No".to_string(), None),
                ];
                (
                    SelectorKind::ExtensionConfirm,
                    title.clone(),
                    Box::new(ListSelector::prompt(title, rows, 0)),
                )
            }
            UiKind::Select => {
                // L4 review §4: an empty `options` list must still OPEN the dialog (Pi's
                // `ExtensionSelectorComponent`, `extension-selector.ts:101-103`, renders whatever
                // it's given including `[]`; Enter is a no-op with nothing selected, and resolution
                // only ever happens via Esc/timeout/signal — same as any other select), not
                // short-circuit to `None` before the guest's dialog is ever shown. `cyrup`'s RPC path
                // (`rpc.rs`) already forwards `options: []` verbatim with no such short-circuit;
                // `ListSelector`/`SelectList` already render an empty list safely (`"No matches"`,
                // `current_value()` never panics) — no special-casing needed here beyond NOT
                // early-returning.
                let picked: Vec<String> = options
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let rows: Vec<(String, String, Option<String>)> =
                    picked.into_iter().map(|o| (o.clone(), o, None)).collect();
                (
                    SelectorKind::ExtensionSelect,
                    prompt.clone(),
                    Box::new(ListSelector::prompt(prompt, rows, 0)),
                )
            }
            UiKind::Input => (
                SelectorKind::ExtensionInput,
                prompt.clone(),
                Box::new(TextInputSelector::new(prompt, placeholder)),
            ),
            // L4 review §3: the DEFAULT is an inline dialog (Pi's `ExtensionEditorComponent`,
            // `extension-editor.ts`), not a teardown to `$EDITOR` — `title` on `prompt`, the seed
            // text (Pi `prefill`) on `message` (L4 review §2's `editor(title, initial)` fix).
            UiKind::Editor => (
                SelectorKind::ExtensionEditor,
                prompt.clone(),
                Box::new(ExtensionEditorSelector::new(prompt, &message)),
            ),
        };
        // Pi's `CountdownTimer` (`countdown-timer.ts:7-38`, wired by `ExtensionSelectorComponent`/
        // `ExtensionInputComponent`): a guest-set `opts.timeout_ms > 0` arms a live 1s-cadence
        // countdown, shown in the title from the INSTANT the dialog opens (Pi calls `onTick`
        // synchronously in its constructor, `countdown-timer.ts:19`) and ticked forward by
        // [`App::tick_extension_dialog_countdown`] — closing the gap where the dialog otherwise never
        // showed the deadline `LiveHostServices::ui_roundtrip` already enforces host-side, and stayed
        // open on screen (stale) after that host-side timeout had already resolved the guest's call.
        let deadline = opts
            .timeout_ms
            .filter(|&ms| ms > 0)
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        if let Some(deadline) = deadline {
            inner.set_title(countdown_title(&base_title, deadline));
        }
        self.open_boxed_selector(selector_kind, inner);
        self.state.pending_ui_reply = Some(PendingUiReply { kind, reply, base_title, deadline });
    }

    /// Advance the open extension-UI dialog's countdown by one tick (Pi's `CountdownTimer`'s 1s
    /// `setInterval`, `countdown-timer.ts:21-30`): live-updates the selector's title with the
    /// remaining seconds, or — once the deadline has passed — auto-resolves the dialog to its
    /// per-kind deny default and closes the slot (Pi's `onExpire` → `onCancelCallback`,
    /// `extension-selector.ts:56`/`extension-input.ts:59`), exactly like an `Esc` cancel
    /// ([`App::handle_selector_key`]'s `Cancel` arm). A stale reply send (the host's OWN independent
    /// `ui_roundtrip` timeout already won the race) is a harmless no-op, same as every other reply
    /// site in this module. A no-op when no extension dialog is open or it has no timeout armed —
    /// callers gate the driving interval on this same condition so it costs nothing otherwise.
    ///
    /// `pub` for the same reason as [`Self::open_extension_dialog`]: `tests/*.rs` calls it directly
    /// to simulate the run loop's 1s tick without needing a real `tokio::time::sleep`.
    pub fn tick_extension_dialog_countdown(&mut self) {
        let Some((base_title, deadline)) = self
            .state
            .pending_ui_reply
            .as_ref()
            .and_then(|p| p.deadline.map(|d| (p.base_title.clone(), d)))
        else {
            return;
        };
        if tokio::time::Instant::now() >= deadline {
            if let Some(pending) = self.state.pending_ui_reply.take() {
                let _ = pending.reply.send(default_ui_reply(pending.kind));
            }
            self.close_selector(true);
        } else if let Some(active) = self.state.selector.as_mut() {
            active.inner.set_title(countdown_title(&base_title, deadline));
        }
    }

    /// Route one key to the active selector and act on the outcome (spec/tui/05 §3.1). `Confirm`
    /// applies the selection by kind and closes the slot; `Cancel` restores the prior theme (if any)
    /// and closes; `Preview` re-themes live without closing. A no-op if no selector is open.
    fn handle_selector_key(&mut self, key: &event::KeyEvent) -> AppAction {
        let Some(active) = self.state.selector.as_mut() else { return AppAction::None };
        let outcome = active.inner.handle(key, &self.state.select_keymap);
        let kind = active.kind;
        match outcome {
            SelectorOutcome::Ignored => AppAction::None,
            SelectorOutcome::Redraw => AppAction::Redraw,
            SelectorOutcome::Preview(value) => {
                // Theme live preview: re-theme the whole UI as the highlight moves
                // (`theme-selector.ts:54-56`). Other kinds never emit `Preview`.
                if kind == SelectorKind::Theme {
                    self.set_theme(UiTheme::builtin(&value));
                }
                AppAction::Redraw
            }
            SelectorOutcome::Confirm(value) => {
                let command = self.confirm_selector(kind, &value);
                self.close_selector(false);
                match command {
                    Some(c) => AppAction::Command(c),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::Apply(payload) => {
                // A `/tree` label save (`e` → `LabelInput` submit, tree_selector.rs) rides an
                // `"{entry_id}\u{1f}{label}"` `Apply` payload; the entry id is a UUID (never contains
                // the separator) so the split is unambiguous. Persist it via the session `set_label`
                // path and keep the slot open (the tree already refreshed its own `has_label` star).
                if kind == SelectorKind::Tree {
                    return match payload.split_once(crate::FIELD_SEP) {
                        Some((entry_id, label)) => AppAction::Command(AppCommand::SetEntryLabel {
                            entry_id: entry_id.to_string(),
                            label: label.to_string(),
                        }),
                        None => AppAction::Redraw,
                    };
                }
                // A `/resume` in-list delete/rename rides a unit-separator-*tagged* `Apply` payload
                // (`session_selector.rs`); decode it first so it never mis-routes to the settings
                // handler. The slot stays open (the selector already mutated its own row list).
                if let Some(action) = SessionSelectorOutcome::parse_apply(&payload) {
                    return match action {
                        SessionSelectorOutcome::Delete(path) => {
                            AppAction::Command(AppCommand::DeleteSession(path))
                        }
                        SessionSelectorOutcome::Rename { path, name } => {
                            AppAction::Command(AppCommand::RenameSession { path, name })
                        }
                        // `Resume` never arrives via `Apply` (it is a `Confirm`); ignore defensively.
                        SessionSelectorOutcome::Resume(_) => AppAction::Redraw,
                    };
                }
                // Otherwise a `/settings` row cycled in place: persist it live, keep the slot open
                // (Pi's settings selector applies on each `onChange`). The payload is `"id\u{1f}value"`.
                match payload.split_once(crate::FIELD_SEP) {
                    Some((id, value)) => AppAction::Command(AppCommand::ApplySetting {
                        id: id.to_string(),
                        value: value.to_string(),
                    }),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::Cancel => {
                // A cancelled extension-UI dialog resolves to its per-kind deny default (Pi's
                // `Esc`-cancelled select yields `undefined`, which `confirm`'s `result === Yes` then
                // reads as `false` — `interactive-mode.ts:2172-2179`) rather than hanging the
                // wasm-suspended guest until `ui_roundtrip`'s timeout (or forever, with none set).
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(default_ui_reply(pending.kind));
                }
                self.close_selector(true);
                AppAction::Redraw
            }
            // A `/settings` submenu row (Pi `SettingItem.submenu`, settings-selector.ts:603-610):
            // replace the settings selector with the nested picker. Only `"theme"` exists today — the
            // theme picker with live preview (`ThemeSubmenu`); an unknown id is a defensive no-op.
            SelectorOutcome::OpenSubmenu(id) => {
                if id == "theme" {
                    self.open_selector(SelectorKind::Theme);
                }
                AppAction::Redraw
            }
            // `Ctrl+G` inside the extension `ui.editor` dialog (L4 review §3) — the actual
            // teardown+spawn+restore needs `&mut self: &mut App` (terminal access), which
            // `Selector::handle` doesn't have; bubble it up as an `AppAction` the run loop's
            // fallible `match` dispatches (mirrors the plain `Ctrl+G`/`AppAction::OpenExternalEditor`
            // arm right next to it).
            SelectorOutcome::OpenExternalEditor => AppAction::OpenExternalEditorForSelector,
        }
    }

    /// Apply a confirmed selection. The three dependency-free selectors (theme/thinking/show-images)
    /// are applied fully in-crate and return `None`; the data-bound selectors return an
    /// [`AppCommand::ConfirmSelection`] so the run loop applies the effect at the session layer (set
    /// model, switch branch, login…).
    fn confirm_selector(&mut self, kind: SelectorKind, value: &str) -> Option<AppCommand> {
        match kind {
            SelectorKind::Theme => {
                self.set_theme(UiTheme::builtin(value));
                self.state.transcript.push_status(format!("theme → {value}"));
                None
            }
            SelectorKind::Thinking => {
                self.state.thinking_level = value.to_string();
                self.state.status.set_thinking_level(value);
                // The editor's rule color is the always-visible thinking-level signal (spec/tui/03
                // §3.3) — keep it in lockstep with the selected level.
                self.state.editor.set_thinking_level(value);
                self.state.transcript.push_status(format!("thinking → {value}"));
                None
            }
            SelectorKind::ShowImages => {
                self.state.show_images = value == "yes";
                // TUI-007: the toggle governs TOOL-RESULT images too (Pi passes `showImages` into
                // every `ToolExecutionComponent`, interactive-mode.ts:3449), not just the editor's
                // attachment strip. Off ⇒ Pi's `[Image: …]` text stand-in.
                self.state.transcript.set_show_images(self.state.show_images);
                let label = if self.state.show_images { "inline" } else { "placeholder" };
                self.state.transcript.push_status(format!("images → {label}"));
                None
            }
            SelectorKind::ExtensionConfirm => {
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(UiReply::Confirm(value == "yes"));
                }
                None
            }
            SelectorKind::ExtensionSelect
            | SelectorKind::ExtensionInput
            | SelectorKind::ExtensionEditor => {
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(UiReply::Text(Some(value.to_string())));
                }
                None
            }
            other => Some(AppCommand::ConfirmSelection { kind: other, value: value.to_string() }),
        }
    }

    /// Execute a session/data-bound [`AppCommand`] against the live [`AgentSession`]
    /// (`setupEditorSubmitHandler` command handlers, interactive-mode.ts:2554-2734). Data-bound
    /// selectors source their rows here (spec/tui/05 §8 late-data population) and open via
    /// [`open_data_selector`](Self::open_data_selector); lifecycle/IO commands call the matching
    /// session method and surface a status line / info block. Errors degrade to a status line.
    ///
    /// KNOWN RESIDUAL (L4 review §2.1): this is still awaited **inline** in `App::run`'s `select!`
    /// loop, unlike [`AppAction::ExtensionShortcut`] (spawned — see that arm's comment for the
    /// deadlock this avoids). None of the match arms below call a guest capability directly, but a
    /// FEW `.await` a session-lifecycle op (`Reload`/`NewSession`/`Import`/the `Session`/`UserMessage`
    /// `ConfirmSelection` switch+fork paths/`Compact`) that dispatches `HostEvent::Session{Start,
    /// Shutdown,BeforeSwitch,BeforeFork,Compact}` to every live extension's hook
    /// (`session.rs` `dispatch_notify`/`vetoed`), and a guest SDK hook handler is handed the SAME
    /// `Ctx` a tool/shortcut handler gets (`cyrup-ext-sdk/src/ctx.rs`), so it COULD call
    /// `ctx.ui().*` mid-hook. If one ever does, this call site would deadlock exactly like the
    /// shortcut path did before the fix above — closing that residual needs restructuring
    /// `execute_command`'s state-mutating match arms so only the actual session/runtime `.await` runs
    /// off-task (mirroring the `bash_rx`/`shortcut_status_rx` channel-back pattern), which is a much
    /// larger refactor deliberately out of scope here. No Pi extension in the wild is known to call a
    /// UI dialog from a session-lifecycle hook (tool/shortcut/command handlers are the realistic
    /// corpus, both of which are deadlock-safe today), so this is a documented, narrow, follow-up-
    /// tracked gap rather than a silently-dropped one.
    pub async fn execute_command(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        use AppCommand as C;
        match cmd {
            C::OpenSelector(SelectorKind::Model) => {
                // The bare `app.model.select` entry point (no search term) — same as `/model` with no
                // argument: the unfiltered picker.
                self.handle_model_command(session, None).await;
            }
            C::OpenSelector(SelectorKind::ScopedModels) => {
                // The scoped-models picker is the bespoke checkbox+reorder selector over the FULL
                // catalog (`scoped-models-selector.ts`): the catalog is every available model; the
                // current scope is `scoped_models()` (empty ⇒ "all enabled", Pi's `enabledIds = null`).
                let catalog: Vec<(String, String, String, Option<String>)> = session
                    .model_catalog()
                    .iter()
                    .map(|m| {
                        (m.id.to_string(), m.name.clone(), m.provider.to_string(), Some(m.provider.to_string()))
                    })
                    .collect();
                if catalog.is_empty() {
                    self.state.transcript.push_status("no models available (configure providers)");
                } else {
                    let scoped: Vec<String> =
                        session.scoped_models().into_iter().map(|sm| sm.model.id.to_string()).collect();
                    // Empty scope ⇒ "all enabled" (None); otherwise the explicit ordered set.
                    let enabled = if scoped.is_empty() { None } else { Some(scoped) };
                    self.open_checkbox_selector(catalog, enabled);
                }
            }
            C::OpenSelector(SelectorKind::UserMessage) => {
                let rows: Vec<(String, String, Option<String>)> = session
                    .user_messages_for_forking()
                    .await
                    .into_iter()
                    .enumerate()
                    .map(|(i, a)| {
                        (a.entry_id.to_string(), a.text.clone(), Some(format!("message {}", i + 1)))
                    })
                    .collect();
                if rows.is_empty() {
                    self.state.transcript.push_status("no user messages to fork from");
                } else {
                    let last = rows.len().saturating_sub(1);
                    self.open_data_selector(SelectorKind::UserMessage, rows, last);
                }
            }
            C::OpenSelector(SelectorKind::Tree) => {
                // `/tree` (tree-selector.ts): the real session DAG flattened via the new
                // `AgentSession::session_dag` getter (feature #2) — nodes with parent/depth/label/kind/
                // fold/leaf/label/timestamp — feeding the connector/fold/filter engine in
                // `tree_selector.rs`. This replaces the flat user-message spine the audit flagged as
                // "data-starved" so the selector renders the actual branch tree.
                let dag = session.session_dag().await;
                if dag.is_empty() {
                    self.state.transcript.push_status("no session history to navigate");
                } else {
                    let nodes: Vec<TreeNode> =
                        dag.iter().map(tree_node_from_dag).collect();
                    let mut tree = TreeSelector::new(nodes);
                    tree.set_keymap(self.state.tree_keymap.clone());
                    self.open_boxed_selector(SelectorKind::Tree, Box::new(tree));
                }
            }
            C::OpenSelector(SelectorKind::Login) => {
                // `/login` (oauth-selector.ts + getLoginProviderOptions, interactive-mode.ts:4594-4617):
                // the api-key-configurable providers are the unique providers in the model catalog,
                // each tagged with its live auth state (stored / env / unconfigured). The oauth
                // subscription device flow is the provider-tail residual; the picker + status UI is
                // built here, and confirming surfaces the chosen provider's next step.
                let auth = &session.services().auth;
                let mut seen = std::collections::BTreeSet::new();
                let mut entries = Vec::new();
                for model in session.model_catalog() {
                    let id = model.provider.to_string();
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let status = auth.get_auth_status(&cyrup_core::ProviderId::from(id.as_str()), None);
                    let state = crate::AuthState::from_status(status.configured, status.source.is_some());
                    entries.push((id, state));
                }
                let rows = crate::provider_rows(entries);
                if rows.is_empty() {
                    self.state.transcript.push_status("no providers available to configure");
                } else {
                    self.open_data_selector(SelectorKind::Login, rows, 0);
                }
            }
            C::OpenSelector(SelectorKind::Logout) => {
                // `/logout` (getLogoutProviderOptions, interactive-mode.ts:4619-4636): only providers
                // with a STORED credential are listed; confirming deletes it. Env/`models.json` config
                // is untouched (Pi's status-line caveat below).
                let auth = &session.services().auth;
                let stored = auth.list().unwrap_or_default();
                let entries: Vec<(String, crate::AuthState)> =
                    stored.into_iter().map(|id| (id, crate::AuthState::Configured)).collect();
                let rows = crate::provider_rows(entries);
                if rows.is_empty() {
                    self.state.transcript.push_status(
                        "no stored credentials to remove (/logout only removes /login credentials; \
                         env vars and models.json are unchanged)",
                    );
                } else {
                    self.open_data_selector(SelectorKind::Logout, rows, 0);
                }
            }
            C::OpenSelector(SelectorKind::Settings) => {
                // `/settings` (settings-selector.ts): the curated toggle/choice grid sourced from the
                // live effective settings. Each row cycles in place on `Enter` and persists via
                // `ApplySetting` (Pi's settings selector applies on `onChange`).
                let rows = settings_rows(session.services().settings.effective(), &self.state.theme.name);
                let inner: Box<dyn Selector> = Box::new(SettingsSelector::new("Settings", rows));
                self.open_boxed_selector(SelectorKind::Settings, inner);
            }
            C::OpenSelector(SelectorKind::Trust) => {
                // `/trust` (trust-selector.ts): the yes/parent/no option list under a cwd + saved-
                // decision header. Confirming writes the trust store (`write_project_trust`).
                let options = session.project_trust_options();
                let cwd = session.services().cwd.display().to_string();
                let saved = session.saved_trust_decision();
                let saved_label = format_saved_trust(&saved);
                let selected = options
                    .iter()
                    .position(|o| {
                        saved.as_ref().is_some_and(|s| {
                            s.decision.is_trusted() == o.trusted
                                && o.saved_path.as_deref() == Some(s.path.as_path())
                        })
                    })
                    .unwrap_or(0);
                let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
                let inner: Box<dyn Selector> = Box::new(TrustSelector::new(
                    cwd,
                    saved_label,
                    session.services().project_trusted,
                    labels,
                    selected,
                ));
                self.open_boxed_selector(SelectorKind::Trust, inner);
            }
            C::OpenSelector(SelectorKind::Session) => {
                // `/resume` (session-selector.ts): the persisted-session list for this cwd, newest
                // first, sourced via the additive `list_sessions` seam. Confirming carries the chosen
                // session file path; the actual runtime swap is driven by the L7 `SessionRuntime`
                // (`switch_session`) once the runtime is threaded into the run loop (residual gap #3).
                let sessions = session.list_sessions();
                if sessions.is_empty() {
                    self.state.transcript.push_status("no saved sessions to resume");
                } else {
                    let current = session.session_id().to_string();
                    let rows: Vec<SessionRow> = sessions
                        .iter()
                        .map(|s| {
                            let label = session_label(s);
                            let is_current = s.id.to_string() == current;
                            let desc = format!(
                                "{} msgs{}",
                                s.message_count,
                                if is_current { " (current)" } else { "" }
                            );
                            // The query-DSL search text (`getSessionSearchText`,
                            // session-selector-search.ts:26): `{id} {name} {allMessagesText} {cwd}`.
                            let search_text = format!(
                                "{} {} {} {}",
                                s.id,
                                s.name.as_deref().unwrap_or(""),
                                s.all_messages_text,
                                s.cwd
                            );
                            SessionRow {
                                path: s.path.display().to_string(),
                                label,
                                name: s.name.clone(),
                                desc: Some(desc),
                                search_text,
                                recency: system_time_nanos(s.modified),
                            }
                        })
                        .collect();
                    let inner: Box<dyn Selector> = Box::new(SessionSelector::new(rows));
                    self.open_boxed_selector(SelectorKind::Session, inner);
                }
            }
            C::OpenSelector(other) => {
                // Any remaining kind has no in-crate sourcing yet; surface the request (no silent drop).
                self.state.transcript.push_status(format!("{} selector unavailable", other.title()));
            }
            C::ConfirmSelection { kind: SelectorKind::Tree, value } => {
                // Confirming a tree row navigates the leaf to that entry (Pi `navigateTree`,
                // agent-session.ts:2704). A user/custom target re-roots at its parent and yields the
                // target text as re-editable `editor_text`; cancel/no-op is surfaced as a status line.
                let entry = cyrup_core::EntryId::from(value.as_str());
                match session.navigate_tree(entry, NavigateTreeOptions::default()).await {
                    Ok(outcome) if outcome.cancelled => {
                        self.state.transcript.push_status("tree navigation cancelled");
                    }
                    Ok(outcome) => {
                        if let Some(text) = outcome.editor_text {
                            self.state.editor.set_text(&text);
                        }
                        // A summarized branch navigation records a branch-summary message
                        // (`branch-summary-message.ts`) into the transcript.
                        if let Some(entry) = outcome.summary_entry {
                            self.state.transcript.push_branch_summary(entry.summary);
                        }
                        self.state.transcript.push_status("navigated session tree");
                    }
                    Err(e) => self.state.transcript.push_status(format!("tree error: {e}")),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Model, value } => {
                match session.set_model(&value).await {
                    Ok(_) => self.state.transcript.push_status(format!("model → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
                }
            }
            C::SetEntryLabel { entry_id, label } => {
                // Persist the `/tree` label edit via the SAME live `set_label` path a loaded
                // extension's `setLabel` uses (`LiveHostServices::set_label` → `manager.append_label`,
                // host_services.rs:866). An empty label removes it (`apply_label` drops empty labels),
                // matching Pi's `value || undefined`. Silently degrades (unknown id / busy), like Pi.
                session.services().host_services.set_label(&entry_id, &label);
                let msg = if label.is_empty() {
                    "label removed".to_string()
                } else {
                    format!("label → {label}")
                };
                self.state.transcript.push_status(msg);
            }
            C::ModelCommand(search) => {
                // `/model [text]` (`handleModelCommand`, interactive-mode.ts:4175-4196): exact match
                // sets directly; a partial (or bare) opens the picker pre-filtered to `search`.
                self.handle_model_command(session, search).await;
            }
            C::CycleModel(direction) => {
                // The cycle set is the scoped models when a scope is active, else the full available
                // catalog (Pi `cycleModel`: `scopedModels.length > 0 ? scoped : available`). Cycling by
                // model id mirrors the `/model` confirm path (`set_model(&value)` above); the footer
                // re-reads the new model off the `ModelChanged` event this triggers.
                let scoped = session.scoped_models();
                let scoped_active = !scoped.is_empty();
                let cycle: Vec<(String, String, String)> = if scoped_active {
                    scoped
                        .iter()
                        .map(|sm| {
                            (sm.model.id.to_string(), sm.model.provider.to_string(), sm.model.name.clone())
                        })
                        .collect()
                } else {
                    session
                        .available_model_catalog()
                        .iter()
                        .map(|m| (m.id.to_string(), m.provider.to_string(), m.name.clone()))
                        .collect()
                };
                if cycle.len() <= 1 {
                    let msg =
                        if scoped_active { "Only one model in scope" } else { "Only one model available" };
                    self.state.transcript.push_status(msg);
                } else {
                    let current = session.model();
                    let n = cycle.len();
                    let cur = cycle.iter().position(|(id, prov, _)| {
                        id == current.model.as_str() && prov == current.provider.as_str()
                    });
                    let next = match direction {
                        CycleDirection::Forward => cur.map_or(0, |i| (i + 1) % n),
                        CycleDirection::Backward => cur.map_or(0, |i| (i + n - 1) % n),
                    };
                    if let Some((id, _prov, name)) = cycle.get(next) {
                        match session.set_model(id).await {
                            Ok(_) => self.state.transcript.push_status(format!("Switched to {name}")),
                            Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
                        }
                    }
                }
            }
            C::CycleThinking => {
                // Pi `cycleThinkingLevel` (interactive-mode.ts:3606-3614): call the session's cycle,
                // which is gated on `supportsThinking()` (agent-session.ts:1599-1608) and walks the
                // model's OWN supported levels (`getSupportedThinkingLevels`, incl. `xhigh` where
                // mapped). `Ok(None)` ⇒ the model does not reason: show the exact Pi status and change
                // NOTHING. `Ok(Some(level))` ⇒ `set_thinking_level` already emitted
                // `ThinkingLevelChanged`, so the footer + editor rule re-color off that event
                // (mirroring Pi's `footer.invalidate()` + `updateEditorBorderColor()`); here we only
                // surface Pi's `Thinking level: {level}` status line.
                match session.cycle_thinking_level().await {
                    Ok(None) => {
                        self.state.transcript.push_status("Current model does not support thinking");
                    }
                    Ok(Some(level)) => {
                        let label = match level {
                            ModelThinkingLevel::Off => "off",
                            ModelThinkingLevel::Minimal => "minimal",
                            ModelThinkingLevel::Low => "low",
                            ModelThinkingLevel::Medium => "medium",
                            ModelThinkingLevel::High => "high",
                            ModelThinkingLevel::Xhigh => "xhigh",
                            ModelThinkingLevel::Max => "max",
                        };
                        self.state.transcript.push_status(format!("Thinking level: {label}"));
                    }
                    Err(e) => self.state.transcript.push_status(format!("thinking error: {e}")),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::ScopedModels, value } => {
                // The checkbox selector confirms with the ordered enabled ids (`\n`-joined), or the
                // `SCOPED_MODELS_ALL` sentinel for "all enabled". Rebuild the scoped set from the
                // catalog and persist via `set_scoped_models` (`scoped-models-selector.ts onPersist`).
                let catalog = session.model_catalog();
                let ordered_ids: Vec<String> = if value == crate::SCOPED_MODELS_ALL {
                    catalog.iter().map(|m| m.id.to_string()).collect()
                } else {
                    value.split('\n').filter(|s| !s.is_empty()).map(str::to_string).collect()
                };
                let scoped: Vec<cyrup_session_svc::ScopedModel> = ordered_ids
                    .iter()
                    .filter_map(|id| catalog.iter().find(|m| m.id.to_string() == *id))
                    .map(|m| cyrup_session_svc::ScopedModel {
                        model: m.clone(),
                        thinking_level: None,
                    })
                    .collect();
                let n = scoped.len();
                session.set_scoped_models(scoped);
                self.state.transcript.push_status(format!("scoped models → {n} enabled"));
            }
            C::ConfirmSelection { kind: SelectorKind::UserMessage, value } => {
                // `/fork` (user-message-selector.ts): fork at the chosen entry. With the runtime
                // threaded in, drive `AgentSessionRuntime::fork` so the runtime swaps to the new
                // branched session and the UI re-binds on the generation bump (Pi `fork`,
                // agent-session-runtime.ts:259); `position:"before"` re-seeds the editor with the
                // anchor text. Without a runtime (SDK/embedder), fall back to the in-place
                // `fork_at_entry` (no swap).
                let entry = cyrup_core::EntryId::from(value.as_str());
                match runtime {
                    Some(rt) => match rt.fork(entry, ForkPosition::Before).await {
                        Ok(r) if r.cancelled => {
                            self.state.transcript.push_status("fork cancelled")
                        }
                        Ok(r) => {
                            if let Some(text) = r.selected_text {
                                self.state.editor.set_text(&text);
                            }
                            self.state.pending_swap_status = Some("forked from message".into());
                        }
                        Err(e) => self.state.transcript.push_status(format!("fork error: {e}")),
                    },
                    None => match session.fork_at_entry(&entry, ForkPosition::Before).await {
                        Ok(_) => self.state.transcript.push_status("forked from message"),
                        Err(e) => self.state.transcript.push_status(format!("fork error: {e}")),
                    },
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Logout, value } => {
                // Delete the stored credential for the chosen provider (Pi `/logout` onSelect →
                // `authStorage.delete`, oauth-selector.ts). A real, in-crate effect against the
                // `AuthStore` the session owns; env/config tiers are untouched.
                let provider = cyrup_core::ProviderId::from(value.as_str());
                match session.services().auth.delete(&provider).await {
                    Ok(()) => self
                        .state
                        .transcript
                        .push_status(format!("removed stored credentials for {value}")),
                    Err(e) => self.state.transcript.push_status(format!("logout error: {e}")),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Login, value } => {
                // The credential write itself — the oauth device/PKCE handshake or the api-key prompt
                // dialog (Pi `showLoginDialog`/`showApiKeyLoginDialog`) — is the provider-tail residual.
                // Surface the chosen provider + its next step so the picker is a real path.
                let name = crate::provider_display_name(&value);
                self.state.transcript.push_status(format!(
                    "{name}: set the API key via `{value}` env var or `models.json`"
                ));
            }
            C::ConfirmSelection { kind: SelectorKind::Trust, value } => {
                // The trust selector confirms with the chosen option INDEX; re-derive the options and
                // persist that option's store updates (Pi `/trust` `onSelect` → trust-store write).
                let options = session.project_trust_options();
                match value.parse::<usize>().ok().and_then(|i| options.get(i)) {
                    Some(opt) => match session.write_project_trust(&opt.updates) {
                        Ok(()) => {
                            let label = if opt.trusted { "trusted" } else { "untrusted" };
                            self.state.transcript.push_status(format!(
                                "project trust → {label} (/reload to apply to this session)"
                            ));
                        }
                        Err(e) => self.state.transcript.push_status(format!("trust error: {e}")),
                    },
                    None => self.state.transcript.push_status("trust selection cancelled"),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Session, value } => {
                // `/resume` swap (handleResumeSession, interactive-mode.ts): switch the runtime to the
                // chosen session file (Pi `switchSession`, agent-session-runtime.ts:193). The runtime
                // asserts the resumed cwd still exists, rebuilds cwd-bound services, and bumps the
                // generation; the UI re-binds on the bump. Without a runtime, surface the path.
                match runtime {
                    Some(rt) => match rt.switch_session(value.clone()).await {
                        Ok(r) if r.cancelled => {
                            self.state.transcript.push_status("resume cancelled")
                        }
                        Ok(_) => {
                            self.state.pending_swap_status = Some(format!("resumed {value}"));
                        }
                        Err(e) => self.state.transcript.push_status(format!("resume error: {e}")),
                    },
                    None => self
                        .state
                        .transcript
                        .push_status(format!("resume {value} (/reload to switch)")),
                }
            }
            C::DeleteSession(path) => {
                // `/resume` in-list delete (`onDeleteSession`): remove the persisted JSONL via the
                // additive `delete_session_file` seam (refuses the active session).
                match session.delete_session_file(std::path::Path::new(&path)) {
                    Ok(()) => self.state.transcript.push_status(format!("deleted session {path}")),
                    Err(e) => self.state.transcript.push_status(format!("delete error: {e}")),
                }
            }
            C::RenameSession { path, name } => {
                // `/resume` in-list rename (`onRenameSession`): persist a `session_info` name on the
                // target file via the additive `rename_session_file` seam.
                match session.rename_session_file(std::path::Path::new(&path), &name).await {
                    Ok(()) => self.state.transcript.push_status(format!("renamed session → {name}")),
                    Err(e) => self.state.transcript.push_status(format!("rename error: {e}")),
                }
            }
            C::ConfirmSelection { kind, value } => {
                self.state.transcript.push_status(format!("{} → {value}", kind.title()));
            }
            C::ApplySetting { id, value } => {
                // Persist a `/settings` toggle/choice live (Global scope; Pi's settings selector
                // writes the global layer). The `/reload` re-reads the effective view.
                let json = parse_setting_value(&value);
                // `outputPad` also takes effect ON SCREEN immediately (Pi `onOutputPadChange` →
                // `this.outputPad = padding` + re-render, interactive-mode.ts:4127-4136), unlike the
                // settings that only rebind on `/reload`: push the new pad into the live transcript so
                // the chat horizontal padding changes the moment the row is cycled.
                if id == "outputPad" {
                    let pad = if value == "0" { 0 } else { 1 };
                    self.state.transcript.set_output_pad(pad);
                }
                // `hideThinkingBlock` likewise takes effect live (Pi `setHideThinkingBlock`,
                // assistant-message.ts:57-62) — on the in-flight reasoning block and on every entry
                // committed after the flip. Pi additionally re-renders the ALREADY-shown assistant
                // messages; cyrup's committed rows have left the render tree for native scrollback
                // (`flush_committed` → `insert_before`), so history keeps the form it committed with.
                if id == "hideThinkingBlock" {
                    self.state.transcript.set_hide_thinking_block(value == "true");
                }
                // The image rows are live too (Pi re-reads them per `ToolExecutionComponent`).
                if id == "terminal.showImages" {
                    self.state.show_images = value == "true";
                    self.state.transcript.set_show_images(self.state.show_images);
                }
                if id == "terminal.imageWidthCells"
                    && let Ok(cells) = value.parse::<u16>()
                {
                    self.state.transcript.set_image_width_cells(cells);
                }
                match session.persist_setting(cyrup_session_svc::SettingsScope::Global, &id, json) {
                    Ok(()) => self.state.transcript.push_status(format!("{id} → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("settings error: {e}")),
                }
            }
            C::Compact(arg) => match session.compact(arg).await {
                // Render the compaction-summary message (`compaction-summary-message.ts`): the
                // `[compaction]` label + `**Compacted from N tokens**` markdown body produced by the
                // op (Pi appends a `CompactionSummaryMessage` after a manual `/compact`).
                Ok(result) => {
                    self.state
                        .transcript
                        .push_compaction_summary(result.tokens_before, result.summary);
                }
                // A refusal (nothing to compact / already compacted / an extension veto) is now an
                // `Err` carrying Pi's reason string, so the status line names WHY instead of the old
                // undifferentiated "nothing to compact".
                Err(e) => self.state.transcript.push_status(format!("compact error: {e}")),
            },
            C::Clone => match session.clone_at(None).await {
                Ok(id) => self.state.transcript.push_status(format!("cloned session → {id}")),
                Err(e) => self.state.transcript.push_status(format!("clone error: {e}")),
            },
            C::Export(arg) => {
                // Format chosen **by extension**, matching Pi (`handleExportCommand`,
                // interactive-mode.ts:5106-5112): a `.jsonl` target writes the raw transcript;
                // every other target (including no path) writes a styled HTML document — HTML is the
                // default. cyrup renders the HTML body in-crate (`export::session_jsonl_to_html`) over
                // the session's own JSONL; the rich tool-card renderer is the L5 residual.
                let is_jsonl =
                    arg.as_deref().is_some_and(|p| p.trim_end().to_ascii_lowercase().ends_with(".jsonl"));
                if is_jsonl {
                    let path = arg.as_deref().map(std::path::Path::new);
                    match session.export_to_jsonl(path).await {
                        Ok(_) => {
                            let label = arg.unwrap_or_default();
                            self.state.transcript.push_status(format!("exported session → {label}"));
                        }
                        Err(e) => self.state.transcript.push_status(format!("export error: {e}")),
                    }
                } else {
                    // Pull the transcript as JSONL (no path ⇒ returned as text), render to HTML, write.
                    match session.export_to_jsonl(None).await {
                        Ok(Some(jsonl)) => {
                            let html = crate::export::session_jsonl_to_html(&jsonl);
                            match &arg {
                                Some(path) => match std::fs::write(path, &html) {
                                    Ok(()) => self
                                        .state
                                        .transcript
                                        .push_status(format!("exported session → {path}")),
                                    Err(e) => self
                                        .state
                                        .transcript
                                        .push_status(format!("export error: {e}")),
                                },
                                None => self.state.transcript.push_block("Session (HTML)", html),
                            }
                        }
                        Ok(None) => self.state.transcript.push_status("exported session"),
                        Err(e) => self.state.transcript.push_status(format!("export error: {e}")),
                    }
                }
            }
            C::SetName(name) => match session.set_session_name(&name).await {
                Ok(()) => self.state.transcript.push_status(format!("session name → {name}")),
                Err(e) => self.state.transcript.push_status(format!("name error: {e}")),
            },
            C::Copy => match session.last_assistant_text().await {
                Some(text) => {
                    let n = text.chars().count();
                    copy_to_clipboard(&text);
                    self.state.transcript.push_status(format!("copied last message ({n} chars)"));
                }
                None => self.state.transcript.push_status("no assistant message to copy"),
            },
            C::SessionInfo => {
                let stats = session.session_stats().await;
                let body = format!(
                    "| Field | Value |\n|-------|-------|\n\
                     | messages | {} |\n| user | {} |\n| assistant | {} |\n\
                     | tool results | {} |\n| input tokens | {} |\n| output tokens | {} |\n\
                     | cache tokens | {} |\n",
                    stats.message_count,
                    stats.user_message_count,
                    stats.assistant_message_count,
                    stats.tool_result_count,
                    stats.input_tokens,
                    stats.output_tokens,
                    stats.cache_tokens,
                );
                self.state.transcript.push_block("Session", body);
            }
            // Session-lifecycle ops drive the `AgentSessionRuntime` (arch-11 §3.4): the op rebuilds
            // the active session + bumps the generation, and the run loop's generation-watch arm
            // re-binds the UI (re-subscribe + reset transcript) → `pending_swap_status`. Without a
            // runtime (SDK/embedder), surface the request so the path is real (no silent drop).
            C::NewSession => match runtime {
                // `/new` (handleClearCommand): start a fresh session in the same cwd (Pi `newSession`).
                Some(rt) => match rt.new_session().await {
                    Ok(r) if r.cancelled => {
                        self.state.transcript.push_status("new session cancelled")
                    }
                    Ok(_) => self.state.pending_swap_status = Some("started a new session".into()),
                    Err(e) => self.state.transcript.push_status(format!("new session error: {e}")),
                },
                None => self.state.transcript.push_status("starting new session…"),
            },
            C::Reload => match runtime {
                // `/reload` (handleReloadCommand): rebuild the active session in place (Pi `reload`,
                // agent-session.ts:2451) — re-reads settings/resources/keybindings, resets the
                // provider, preserves the persisted transcript.
                Some(rt) => match rt.reload(None).await {
                    Ok(()) => self.state.pending_swap_status = Some("reloaded resources".into()),
                    Err(e) => self.state.transcript.push_status(format!("reload error: {e}")),
                },
                None => self.state.transcript.push_status("reloading resources…"),
            },
            C::Import(p) => match (runtime, p) {
                // `/import <path>` (handleImportCommand): copy + resume a JSONL session (Pi
                // `importFromJsonl`, agent-session-runtime.ts:353).
                (Some(rt), Some(path)) => match rt.import_from_jsonl(path.clone(), None).await {
                    Ok(r) if r.cancelled => self.state.transcript.push_status("import cancelled"),
                    Ok(_) => {
                        self.state.pending_swap_status = Some(format!("imported session {path}"))
                    }
                    Err(e) => self.state.transcript.push_status(format!("import error: {e}")),
                },
                (Some(_), None) => self.state.transcript.push_status("usage: /import <path>"),
                (None, p) => self
                    .state
                    .transcript
                    .push_status(format!("importing session {}", p.unwrap_or_default())),
            },
            C::Share => self.share_session(session).await,
        }
    }

    /// `/share` (`handleShareCommand`, interactive-mode.ts:5191): export the session to HTML, write a
    /// temp file, then shell `gh gist create --public=false <file>` behind a [`BorderedLoader`] and
    /// surface the resulting gist URL. `gh` missing / logged-out / failing degrades to a status line
    /// (Pi's `showError` paths). Fully in-crate (the HTML body is rendered by [`crate::export`]).
    async fn share_session(&mut self, session: &Arc<AgentSession>) {
        use tokio::process::Command;
        // Render the session HTML over its own JSONL (the same body `/export` writes).
        let html = match session.export_to_jsonl(None).await {
            Ok(Some(jsonl)) => crate::export::session_jsonl_to_html(&jsonl),
            Ok(None) => {
                self.state.transcript.push_status("nothing to share (empty session)");
                return;
            }
            Err(e) => {
                self.state.transcript.push_status(format!("share export error: {e}"));
                return;
            }
        };
        let tmp = std::env::temp_dir().join(format!("cyrup-session-{}.html", session.session_id()));
        if let Err(e) = std::fs::write(&tmp, html.as_bytes()) {
            self.state.transcript.push_status(format!("share write error: {e}"));
            return;
        }
        // Show the bordered loader in the editor slot while gh runs (Pi's `BorderedLoader`).
        self.state.loader = Some(crate::chrome::BorderedLoader::cancellable(
            "Creating gist…",
            self.state.keymap.key_label(Action::Interrupt).unwrap_or_else(|| "esc".into()),
        ));
        let result = Command::new("gh")
            .args(["gist", "create", "--public=false"])
            .arg(&tmp)
            .output()
            .await;
        self.state.loader = None;
        let _ = std::fs::remove_file(&tmp);
        match result {
            Ok(out) if out.status.success() => {
                let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if url.is_empty() {
                    self.state.transcript.push_status("gist created (no URL returned by gh)");
                } else {
                    self.state.transcript.push_block("Shared session", url);
                }
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let msg = err.trim();
                let detail = if msg.is_empty() { "gh gist create failed" } else { msg };
                self.state.transcript.push_status(format!("share error: {detail}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self
                .state
                .transcript
                .push_status("GitHub CLI (gh) is not installed — see https://cli.github.com/"),
            Err(e) => self.state.transcript.push_status(format!("share error: {e}")),
        }
    }

    /// Close the selector slot and restore the editor (spec/tui/05 §7 `done()`). When `cancelled` and
    /// a theme was being previewed, the prior theme is restored first.
    fn close_selector(&mut self, cancelled: bool) {
        if let Some(active) = self.state.selector.take() {
            if cancelled && let Some(theme) = active.restore_theme {
                self.set_theme(theme);
            }
            self.state.editor.set_text(&active.saved_editor);
        }
    }

    /// Fold an `AgentSessionEvent` into the UI state.
    ///
    /// Decodes tool names + error flag, model changes, queue depth, compaction, the live streaming
    /// **delta** text (`MessageUpdate` → [`Self::ingest_stream_event`] →
    /// [`TranscriptView::push_assistant_delta`](crate::transcript::TranscriptView::push_assistant_delta)),
    /// and the **terminal** assistant message (recovered via `StreamEvent::terminal_message()`, which
    /// yields a `&cyrup_core::AssistantMessage`). `cyrup-provider` is a direct dependency, so the
    /// token-by-token render (gap 1) is live, not deferred.
    pub fn ingest_event(&mut self, ev: &AgentSessionEvent) {
        self.ingest_event_rendered(ev, None);
    }

    /// [`Self::ingest_event`], first giving the loaded extensions a chance to RENDER the event
    /// (EXT-006). This is the production fold the interactive run loop uses; the sync
    /// [`Self::ingest_event`] is the no-extensions shorthand.
    ///
    /// Pi resolves a renderer at the point of display — `extensionRunner.getMessageRenderer(...)`
    /// for a custom message (interactive-mode.ts:3324-3336) and the per-tool `renderCall`/
    /// `renderResult` for a tool row (components/tool-execution.ts:81-112). cyrup's fold is sync
    /// (it mutates `&mut self` from a `select!` arm) while a guest renderer is an async wasm call,
    /// so the renderer runs FIRST and its text rides into the fold.
    pub async fn ingest_event_with_extensions(
        &mut self,
        ev: &AgentSessionEvent,
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        let rendered = extension_render(ext_host, ev).await;
        self.ingest_event_rendered(ev, rendered);
    }

    fn ingest_event_rendered(&mut self, ev: &AgentSessionEvent, rendered: Option<String>) {
        match ev {
            AgentSessionEvent::AgentStart => {
                self.state.status.set_streaming(true);
                self.state.indicator.working();
            }
            AgentSessionEvent::AgentEnd { .. } => {
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                // Reasoning commits BEFORE the answer text so the scrollback order matches Pi's
                // content walk (thinking section, then the assistant markdown).
                self.state.transcript.commit_thinking(None);
                self.state.transcript.commit_assistant(None);
                // Commit the turn's live tool executions into scrollback (`tool-execution.ts` tools
                // persist through the turn, then scroll up as committed history).
                self.state.transcript.commit_tools();
            }
            // SEAM-005 `agent_settled` (Pi interactive-mode.ts:3137): the run has fully settled —
            // no retry, post-run compaction or queued continuation will follow. Pi's interactive
            // mode does exactly ONE thing here, `await this.checkShutdownRequested()`; the visual
            // teardown already happened on `agent_end` above. That shutdown check lives in the
            // async event-loop arm (`run`, the `events.next()` branch) rather than in this sync
            // fold, which cannot `await` or return control to the caller — so this arm is a
            // deliberate no-op, NOT a missing case.
            AgentSessionEvent::AgentSettled => {}
            AgentSessionEvent::TurnStart | AgentSessionEvent::TurnEnd { .. } => {}
            // A finished message: an extension `Custom` message renders as a distinct labeled block
            // (`custom-message.ts`, interactive-mode.ts:3083). Core user/assistant text is already
            // surfaced via the user echo + streaming-delta path, so only `Custom` is folded here (on
            // `MessageEnd` so it commits once, not twice with `MessageStart`).
            AgentSessionEvent::MessageStart { .. } => {}
            AgentSessionEvent::MessageEnd { .. } => {
                // The `AgentMessage` type lives in `cyrup-agent` (a dev-dep here, not a direct dep), so
                // the `Custom` arm is detected via its serde projection (`tag = "role"`,
                // `rename_all_fields = camelCase`) rather than a direct match — no dependency ripple.
                if let Some((kind, body)) = custom_message_from_event(ev) {
                    // EXT-006: `rendered` is the text the extension that registered a renderer for
                    // this custom type produced; absent one it is `None` and the default
                    // `[kind] body` framing draws (Pi `CustomMessageComponent`).
                    self.state.transcript.push_custom_message_rendered(kind, body, rendered);
                }
            }
            AgentSessionEvent::MessageUpdate { assistant_message_event, .. } => {
                self.ingest_stream_event(assistant_message_event);
            }
            AgentSessionEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                // Hand the raw call args to the transcript so each built-in renders its Pi-specific
                // `renderCall` header (path+range / `$ command` / `/pattern/` / …), not a generic
                // one-liner (transcript.rs `tool_lines` dispatch).
                // The `toolCallId` is what the matching `ToolExecutionEnd` is paired back by — Pi
                // files the component under it (`pendingTools.set(event.toolCallId, component)`,
                // interactive-mode.ts:3096). A turn that batches two `read`s cannot be resolved by
                // tool name.
                // EXT-006: an extension that declared a renderer for THIS tool supplies the call
                // header; `None` keeps the built-in per-tool dispatch.
                self.state.transcript.push_tool_start_rendered(
                    tool_name.clone(),
                    Some(tool_call_id.as_str().to_string()),
                    args.clone(),
                    rendered,
                );
            }
            AgentSessionEvent::ToolExecutionUpdate { tool_call_id, partial_result, .. } => {
                // Pi: `this.pendingTools.get(event.toolCallId)` (interactive-mode.ts:3104).
                self.state
                    .transcript
                    .push_tool_update(Some(tool_call_id.as_str()), Some(partial_result.clone()));
            }
            AgentSessionEvent::ToolExecutionEnd { tool_call_id, tool_name, is_error, result } => {
                // The full `{content, details, terminate}` result flows through so `renderResult` can
                // reach each tool's `details` (edit `diff`, bash/read truncation, …), and the
                // `toolCallId` routes it to the run that made THIS call (`:3113`).
                self.state.transcript.push_tool_end_rendered(
                    tool_name.clone(),
                    Some(tool_call_id.as_str()),
                    *is_error,
                    Some(result.clone()),
                    rendered,
                );
                // Progressively flush finished tools to native scrollback mid-turn so the inline
                // viewport holds only the running tail, not the whole turn's tool stack (the
                // SCREEN-FILL disaster). The finished tool leaves `active_tools` here; the
                // draw-after-every-event (`flush_committed` → `insert_before`) lands it above the
                // viewport on the very next frame, and `terminal.draw` renders the tail without it —
                // an atomic handoff, no duplicate/flash. Mirrors Pi's completed `tool-execution.ts`
                // components scrolling up into native history as the turn proceeds.
                self.state.transcript.commit_finished_leading_tools();
            }
            AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                self.state.status.set_queued(steering.len().saturating_add(follow_up.len()));
            }
            AgentSessionEvent::CompactionStart { reason } => {
                // Pi's exact status copy (status-indicator.ts:80-82): a MANUAL `/compact` reads
                // "Compacting context…"; an automatic compaction reads "Auto-compacting…", prefixed
                // "Context overflow detected, " when the overflow path triggered it (item #9). The
                // ` (<key> to cancel)` suffix is appended by the band from the live keymap.
                let msg = match reason {
                    CompactionReason::Manual => "Compacting context…".to_string(),
                    CompactionReason::Overflow => {
                        "Context overflow detected, Auto-compacting…".to_string()
                    }
                    CompactionReason::Threshold => "Auto-compacting…".to_string(),
                };
                self.state.indicator.set(IndicatorKind::Compaction, Some(msg.clone()));
                self.state.transcript.push_status(msg);
            }
            AgentSessionEvent::CompactionEnd { .. } => {
                // Back to working if the turn is still streaming, else idle.
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
                self.state.transcript.push_status("compaction complete");
            }
            AgentSessionEvent::AutoRetryStart { attempt, max_attempts, delay_ms, .. } => {
                // Pi's exact retry copy (status-indicator.ts:46-47): "Retrying (a/max) in Ns…" where
                // N is the backoff delay in whole seconds, rounded up (item #9). The ` (<key> to
                // cancel)` suffix is appended by the band from the live keymap.
                let seconds = delay_ms.div_ceil(1000);
                let msg = format!("Retrying ({attempt}/{max_attempts}) in {seconds}s…");
                self.state.indicator.set(IndicatorKind::Retry, Some(msg.clone()));
                self.state.transcript.push_status(msg);
            }
            AgentSessionEvent::AutoRetryEnd { success, .. } => {
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
                self.state
                    .transcript
                    .push_status(if *success { "retry succeeded" } else { "retry ended" });
            }
            AgentSessionEvent::ModelChanged { provider, model } => {
                let label = format!("{provider}/{model}");
                self.state.status.set_model(label.clone());
                // Feed the provider into the footer right cluster (`(provider)` prefix, footer.ts:191).
                self.state.status.set_provider(Some(provider.clone()));
                self.state.transcript.push_status(format!("model → {label}"));
            }
            AgentSessionEvent::ThinkingLevelChanged { level } => {
                // Pi's `thinking_level_changed` handler (interactive-mode.ts:2804-2807) only
                // `footer.invalidate()` + `updateEditorBorderColor()` — NO status line (the acting
                // command, e.g. Shift+Tab's `C::CycleThinking`, owns the status). Mirror the level into
                // the footer right cluster (`• {level}`, footer.ts:186-188), the editor's rule color
                // (spec/tui/03 §3.3), and the TUI's cached level so `/debug` reflects the authoritative
                // session state.
                self.state.thinking_level = level.clone();
                self.state.status.set_thinking_level(level.clone());
                self.state.editor.set_thinking_level(level);
            }
            AgentSessionEvent::SessionInfoChanged { name } => {
                // Pi `interactive-mode.ts:2784` mirrors the renamed session into the header/status.
                let label = name.clone().unwrap_or_default();
                self.state.transcript.push_status(format!("session renamed → {label}"));
            }
            AgentSessionEvent::EntryAppended { entry } => {
                // A loaded extension appended a custom (non-LLM) entry to the tree (Pi
                // `entry_appended`, agent-session.ts:140). Surface its type as a status line.
                let ty = entry
                    .get("type")
                    .or_else(|| entry.get("customType"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("custom");
                self.state.transcript.push_status(format!("entry appended → {ty}"));
            }
            // Session lifecycle (runtime replacement, arch-11 §3.4): surface a status line. The TUI
            // re-subscribes to the runtime's new generation on `SessionReplaced` (R-11-021); the
            // re-subscription itself is driven by the app's runtime watch, not this transcript hook.
            AgentSessionEvent::SessionStart { reason, .. } => {
                self.state.transcript.push_status(format!("session started ({reason})"));
            }
            AgentSessionEvent::SessionShutdown { reason } => {
                self.state.transcript.push_status(format!("session shutdown ({reason})"));
            }
            AgentSessionEvent::SessionReplaced { .. } => {
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
            }
        }
    }

    /// Fold one streaming `StreamEvent` (the `assistantMessageEvent` payload of a `MessageUpdate`,
    /// session-svc `event.rs:111`) into the transcript — the live token-by-token render (gap 1).
    ///
    /// `TextDelta { delta, .. }` (provider `stream.rs:306`) is appended to the in-flight streaming
    /// buffer via [`TranscriptView::push_assistant_delta`], so the viewport grows a character at a
    /// time exactly like Pi's interactive stream. A terminal event (`Done`/`Error`, recoverable via
    /// [`StreamEvent::terminal_message`]) replaces the partial with the authoritative
    /// `AssistantMessage` text and records its token usage in the footer.
    ///
    /// `ThinkingDelta { delta, .. }` (provider `stream.rs:413`) grows the separate live *reasoning*
    /// block via [`TranscriptView::push_thinking_delta`]; the terminal event commits the message's
    /// authoritative `thinking` blocks ([`thinking_text`]) ahead of the answer text, matching Pi's
    /// in-order content walk (`assistant-message.ts:115-166`). The remaining non-text frames
    /// (start/text-start/text-end/thinking-start/thinking-end/toolcall*) carry only the running
    /// `partial`, whose content already reaches us via the deltas + the terminal, so nothing is
    /// rendered for them.
    ///
    /// A terminal whose `stop_reason` is not a clean stop also appends Pi's error-styled
    /// incomplete/failed-turn notice ([`stop_reason_notice`], `assistant-message.ts:175-201`).
    fn ingest_stream_event(&mut self, ev: &StreamEvent) {
        match ev {
            StreamEvent::TextDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_assistant_delta(delta);
                }
            }
            // Reasoning deltas (provider `stream.rs:413`) grow their own live block above the
            // answer text, exactly as Pi renders the turn's `thinking` content
            // (`assistant-message.ts:115-166`). `ThinkingStart`/`ThinkingEnd` carry no incremental
            // text of their own — the authoritative blocks arrive with the terminal message below.
            StreamEvent::ThinkingDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_thinking_delta(delta);
                }
            }
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => {
                // Commit the reasoning FIRST (Pi walks content in order and thinking precedes the
                // answer), preferring the terminal message's authoritative `thinking` blocks over
                // whatever streamed — a redacted/summarised block only ever arrives terminally.
                let thinking = thinking_text(&message.content);
                if thinking.is_empty() {
                    self.state.transcript.commit_thinking(None);
                } else {
                    self.state.transcript.commit_thinking(Some(thinking));
                }
                let text = content_text(&message.content);
                if text.is_empty() {
                    // Pure tool-use / empty terminal: keep any streamed partial; `AgentEnd` commits it.
                    self.state.transcript.commit_assistant(None);
                } else {
                    self.state.transcript.commit_assistant(Some(text));
                }
                let tokens = message.usage.total_tokens;
                if tokens > 0 {
                    self.state.status.set_tokens(tokens);
                }
                // Accumulate the turn into the cumulative session footer totals (footer.ts:86-107).
                self.state.status.add_usage(&message.usage);
                // A turn that did not finish cleanly gets Pi's error-styled footer notice
                // (assistant-message.ts:175-201) — otherwise a 5xx, an abort or a max-token
                // truncation would end the turn with no explanation at all.
                if let Some(notice) = stop_reason_notice(message) {
                    self.state.transcript.push_error(notice);
                }
            }
            _ => {}
        }
    }
}

/// The `error`-styled notice Pi appends after an assistant turn that did not finish cleanly
/// (`assistant-message.ts:175-201`), or `None` for a clean turn.
///
/// * `length` → the max-output-token sentence, emitted **unconditionally**: a length stop can land
///   before a tool call is complete, so it is surfaced even on a tool turn (`:177`).
/// * `aborted` / `error` → emitted only when the message carries NO `toolCall` content (`:189`),
///   because for those the tool-execution component already reports the failure.
/// * `aborted` shows `errorMessage` unless it is the internal `Request was aborted` sentinel, in
///   which case the user-facing wording is `Operation aborted` (`:190-197`).
/// * `error` shows `Error: {errorMessage || "Unknown error"}` (`:198-201`).
fn stop_reason_notice(message: &cyrup_core::AssistantMessage) -> Option<String> {
    use cyrup_core::StopReason;
    if message.stop_reason == StopReason::Length {
        return Some(
            "Error: Model stopped because it reached the maximum output token limit. \
             The response may be incomplete."
                .to_string(),
        );
    }
    let has_tool_calls =
        message.content.iter().any(|c| matches!(c, cyrup_core::Content::ToolCall(_)));
    if has_tool_calls {
        return None;
    }
    match message.stop_reason {
        StopReason::Aborted => Some(match message.error_message.as_deref() {
            Some(m) if !m.is_empty() && m != "Request was aborted" => m.to_string(),
            _ => "Operation aborted".to_string(),
        }),
        StopReason::Error => Some(format!(
            "Error: {}",
            match message.error_message.as_deref() {
                Some(m) if !m.is_empty() => m,
                _ => "Unknown error",
            }
        )),
        StopReason::Stop | StopReason::Length | StopReason::ToolUse => None,
    }
}

/// Flatten a styled [`Line`] into its plain text (concatenated span content).
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Copy `text` to the system clipboard best-effort via the platform CLI (`pbcopy` on macOS, `xclip`/
/// `wl-copy` on Linux). No new dependency and no unsafe — a missing tool is silently ignored
/// (`handleCopyCommand` clipboard write, interactive-mode.ts:5285). Unix-only.
#[cfg(unix)]
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let candidates: &[(&str, &[&str])] =
        &[("pbcopy", &[]), ("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])];
    for (bin, args) in candidates {
        let child = std::process::Command::new(bin)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
}

/// No-op clipboard write on non-unix targets (the platform CLI tools are unix-only here).
#[cfg(not(unix))]
fn copy_to_clipboard(_text: &str) {}

/// Read a system-clipboard image and materialize it as a PNG temp file, returning its path (Pi
/// `readClipboardImage` + the temp-file write of `handleClipboardImagePaste`,
/// interactive-mode.ts:2544-2549 / `utils/clipboard-image.ts`). `arboard` is the faithful Rust analog
/// of Pi's native clipboard module (`utils/clipboard-native.ts`): NSPasteboard on macOS, X11/Wayland on
/// Linux. `get_image` hands back an RGBA8 raster (`arboard::ImageData`), which is re-encoded to PNG with
/// the in-tree `image` crate and written to `cyrup-clipboard-<uuid>.png` in the OS temp dir (Pi's
/// `pi-clipboard-<randomUUID>.<ext>`, always PNG here since the raster is already decoded).
///
/// Returns `None` when the clipboard holds no image (Pi's `clipboard.hasImage()` gate) or on ANY
/// clipboard/decode/encode/IO error — mirroring Pi's `catch {}` silent-ignore (no clipboard access, a
/// headless/permission-denied session, a zero-area raster, …) — so a bare Ctrl+V never disrupts the
/// editor and simply falls through to normal text handling.
fn read_clipboard_image_to_temp() -> Option<std::path::PathBuf> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let img = clipboard.get_image().ok()?;
    let width = u32::try_from(img.width).ok()?;
    let height = u32::try_from(img.height).ok()?;
    // `arboard::ImageData::bytes` is an RGBA8 raster; `from_raw` returns `None` if the buffer length
    // does not match `width * height * 4`, guarding a malformed clipboard payload without panicking.
    let raster = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?;
    let path =
        std::env::temp_dir().join(format!("cyrup-clipboard-{}.png", uuid::Uuid::now_v7()));
    raster.save_with_format(&path, image::ImageFormat::Png).ok()?;
    Some(path)
}

/// Truncate a one-line summary to a sane length (avoid overrunning the marker line).
/// Detect a `Custom`-role [`cyrup_agent::AgentMessage`] from its serde projection and return its
/// `(kind, body)` for [`TranscriptView::push_custom_message`](crate::transcript::TranscriptView::push_custom_message).
/// `AgentMessage` is only a dev-dependency here, so the message is inspected through `serde_json`
/// (`{"role":"custom","kind":…,"payload":…}`) instead of a direct pattern match — no dep ripple.
/// Returns `None` for any non-custom (core user/assistant/toolResult) message.
/// Whether the interactive host should exit NOW because a loaded extension called `ctx.shutdown()`
/// (EXT-005 / SEAM-005).
///
/// Pi checks a pending shutdown at exactly two moments, and cyrup's run loop calls this at both:
///
/// * `at_settle` — the `agent_settled` arm, `case "agent_settled": await
///   this.checkShutdownRequested()` (interactive-mode.ts:3137-3138). A settle means the whole run
///   is over (no retry, no post-run compaction, no queued continuation), so no idle re-check is
///   needed or wanted;
/// * otherwise — the `shutdownHandler` Pi binds in `bindExtensions`,
///   `this.shutdownRequested = true; if (this.session.isIdle) { void this.shutdown(); }`
///   (interactive-mode.ts:1753-1757). This is what makes Pi's own canonical example,
///   `examples/extensions/shutdown-command.ts` — a `/quit` COMMAND that never starts a run — exit
///   at all; gating solely on a settle would strand it forever.
///
/// Kept as a named predicate rather than an inline condition so it is testable without driving a
/// real terminal event loop.
pub fn should_honor_extension_shutdown(
    session: &cyrup_session_svc::AgentSession,
    at_settle: bool,
) -> bool {
    session.shutdown_requested() && (at_settle || session.is_idle())
}

/// How long the fold waits for an extension renderer before falling back to the built-in framing.
///
/// A renderer is a presentation concern on the interactive event path; it must never be able to
/// wedge the frame. The call runs on its OWN task (not inline in the `select!` arm) for the same
/// reason `AppAction::ExtensionShortcut` spawns: a guest handler may synchronously block on a
/// `ui.{confirm,input,…}` capability whose reply only THIS loop can deliver, and awaiting it inline
/// would be a genuine self-deadlock. Spawn + bounded wait makes the worst case "the block draws
/// with its built-in renderer", never a hang.
const EXTENSION_RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Ask the loaded extensions to render this event, if any registered a renderer for it (EXT-006).
///
/// * a custom message → the extension that registered `custom_type` (Pi `getMessageRenderer`,
///   runner.ts:579-587, resolved at interactive-mode.ts:3326);
/// * a tool start/end → the extension that declared a renderer for that TOOL NAME (Pi's per-tool
///   `renderCall`/`renderResult`, tool-execution.ts:81-112).
///
/// `None` for every other event, and for any event whose key has no registered renderer — the cheap
/// SYNC `has_*_renderer` pre-check runs first so the common path pays nothing.
pub async fn extension_render(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ev: &AgentSessionEvent,
) -> Option<String> {
    enum Which {
        Message(String, serde_json::Value),
        ToolCall(String, serde_json::Value),
        ToolResult(String, serde_json::Value),
    }
    let which = match ev {
        AgentSessionEvent::MessageEnd { .. } => {
            let (kind, _) = custom_message_from_event(ev)?;
            if !ext_host.has_message_renderer(&kind) {
                return None;
            }
            let message = serde_json::to_value(ev).ok()?.get("message")?.clone();
            Which::Message(kind, message)
        }
        AgentSessionEvent::ToolExecutionStart { tool_name, args, .. } => {
            if !ext_host.has_tool_renderer(tool_name) {
                return None;
            }
            Which::ToolCall(tool_name.clone(), args.clone())
        }
        AgentSessionEvent::ToolExecutionEnd { tool_name, result, .. } => {
            if !ext_host.has_tool_renderer(tool_name) {
                return None;
            }
            Which::ToolResult(tool_name.clone(), result.clone())
        }
        _ => return None,
    };
    let host = ext_host.clone();
    let task = tokio::spawn(async move {
        match which {
            Which::Message(key, payload) => host.render_message_call(&key, &payload).await,
            Which::ToolCall(key, payload) => host.render_tool_call(&key, &payload).await,
            Which::ToolResult(key, payload) => host.render_tool_result(&key, &payload).await,
        }
    });
    let abort = task.abort_handle();
    match tokio::time::timeout(EXTENSION_RENDER_TIMEOUT, task).await {
        Ok(Ok(v)) => v.map(|v| rendered_text(&v)),
        // A panicking renderer task degrades to the built-in framing, like a faulting one.
        Ok(Err(_)) => None,
        Err(_) => {
            // Cancel the wedged call rather than detaching it: dropping a `JoinHandle` only
            // detaches, and a renderer that blocks once will block again on the next event, so
            // detached tasks would pile up behind the instance's store lock.
            abort.abort();
            None
        }
    }
}

/// How deep a widget tree may nest before the flattener gives up (a guest can hand the host any
/// JSON, including a pathologically deep one; the flattener must terminate on adversarial input).
const MAX_WIDGET_DEPTH: usize = 16;

/// Flatten a renderer's returned JSON — a SERIALIZED WIDGET TREE — into the display text the
/// transcript draws.
///
/// This is the host half of the `render-call`/`render-result` contract documented in
/// `cyrup-ext/wit/world.wit`. Pi's renderers return an in-process `pi-tui` `Component` which the
/// interactive mode adds as a child of `CustomMessageComponent`/`ToolExecutionComponent`
/// (`components/custom-message.ts:66-81`, `components/tool-execution.ts:81-112`); nothing is ever
/// stringified. A WASM guest cannot hand back a live object, so cyrup's wire analog is the
/// component tree SERIALIZED, and the host is what turns it back into rows — the exact step that
/// was missing: every non-string return used to be pretty-printed, so a guest following cyrup's own
/// SDK example (`{"widget":"text","text":…}`) drew a raw JSON blob where Pi draws the component.
///
/// The vocabulary mirrors the `pi-tui` components a renderer actually returns (`packages/tui/src/
/// index.ts:13-32`); it is duplicated verbatim in both WIT world copies and constructed by
/// `cyrup_ext_sdk::widget` on the guest side:
///
/// | JSON                                              | Pi component      |
/// |---------------------------------------------------|-------------------|
/// | `"…"` (a bare string)                              | `Text` (degenerate) |
/// | `{"widget":"text","text":"…"}`                     | `Text` — the dominant shape |
/// | `{"widget":"markdown","text":"…"}`                 | `Markdown`        |
/// | `{"widget":"truncated-text","text":"…"}`           | `TruncatedText`   |
/// | `{"widget":"spacer","lines":n}` (default 1)        | `Spacer`          |
/// | `{"widget":"box"\|"container","children":[…]}`     | `Box` / `Container` — stacked |
/// | `{"widget":"hstack","children":[…]}`               | `HStack` — joined on one row |
/// | `[…]` (a bare array)                               | shorthand for a stack |
///
/// Anything the vocabulary does not cover — an unknown `widget` tag, a missing tag, a tree deeper
/// than [`MAX_WIDGET_DEPTH`] — falls back to the pretty-printed JSON rather than being dropped, so
/// an author who mistypes a node SEES the node instead of a blank row. The fallback applies to the
/// WHOLE tree, not the offending node, so the JSON on screen is the one the guest actually returned.
fn rendered_text(v: &serde_json::Value) -> String {
    flatten_widget(v, 0)
        .unwrap_or_else(|| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
}

/// One node of [`rendered_text`]'s widget tree. `None` = "not a widget I know", which the caller
/// turns into the whole-tree JSON fallback.
fn flatten_widget(v: &serde_json::Value, depth: usize) -> Option<String> {
    use serde_json::Value;
    if depth > MAX_WIDGET_DEPTH {
        return None;
    }
    match v {
        // A bare string is the degenerate `Text` node: a renderer that just wants to hand back the
        // lines it wants drawn should not have to wrap them.
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => flatten_children(items, depth, "\n"),
        Value::Object(o) => {
            let text = || o.get("text").and_then(Value::as_str).unwrap_or("").to_string();
            let children = |sep: &str| match o.get("children") {
                Some(Value::Array(items)) => flatten_children(items, depth, sep),
                // A container with no children is an empty row, not an error (Pi's `Container`
                // renders nothing until something is added to it).
                None => Some(String::new()),
                Some(_) => None,
            };
            match o.get("widget").and_then(Value::as_str)? {
                "text" | "markdown" | "truncated-text" => Some(text()),
                "spacer" => {
                    // `n` blank rows = a string of `n - 1` newlines (one empty row needs no
                    // separator). Clamped so a guest cannot ask the transcript for a million rows.
                    let n = o.get("lines").and_then(Value::as_u64).unwrap_or(1).min(64) as usize;
                    Some("\n".repeat(n.saturating_sub(1)))
                }
                "box" | "container" => children("\n"),
                "hstack" => children(""),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Flatten every child, joining with `sep`. One unrecognized child fails the WHOLE tree so the
/// caller's JSON fallback shows the guest's actual return rather than a half-rendered tree with a
/// silently missing row.
fn flatten_children(items: &[serde_json::Value], depth: usize, sep: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        out.push(flatten_widget(item, depth.saturating_add(1))?);
    }
    Some(out.join(sep))
}

fn custom_message_from_event(ev: &AgentSessionEvent) -> Option<(String, String)> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("custom") {
        return None;
    }
    let kind =
        message.get("kind").and_then(serde_json::Value::as_str).unwrap_or("custom").to_string();
    let body = message.get("payload").map(custom_message_text).unwrap_or_default();
    Some((kind, body))
}

/// Extract display text from a `Custom` message payload (`string | (Text|Image)[]`, mirroring Pi's
/// `getCustomMessageText`): a JSON string is used verbatim; an array joins its `{text}` parts; any
/// other shape yields the empty string (rendered as a bare label).
fn custom_message_text(payload: &serde_json::Value) -> String {
    match payload {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Collapse a multi-line string to a single space-joined line, truncated to 80 graphemes with an
/// ellipsis (`truncateSummary` — selector descriptions, tree previews).
fn truncate_summary(s: &str) -> String {
    const MAX: usize = 80;
    let one_line = s.replace(['\n', '\r', '\t'], " ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        let head: String = one_line.chars().take(MAX.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Project a flattened [`SessionDagNode`] (feature #2) into the tree selector's [`TreeNode`]: map the
/// UI-agnostic [`SessionDagKind`] to the render [`TreeKind`] glyph, carry depth/label/fold/leaf/label,
/// and use the leaf marker (`◀`) as the time label so the active branch tip is visible in the row.
/// Build the `/model` picker rows from the live session (feature #1): the FULL multi-provider registry
/// filtered to CONFIGURED providers (Pi `modelRegistry.getAvailable()`, model-selector.ts:152 +
/// model-registry.ts:644), each row tagged with its provider, whether it is the active model, and
/// whether it is in the scoped set (drives the `⇥` scope filter). `together` appears once
/// `TOGETHER_API_KEY` is set; the offline faux default stays selectable. Shared by the bare picker and
/// the `/model <text>` exact-match/pre-filter path so both see the identical catalog.
fn model_entries(session: &AgentSession) -> Vec<ModelEntry> {
    let current = session.model();
    let scoped: std::collections::HashSet<String> =
        session.scoped_models().into_iter().map(|sm| sm.model.id.to_string()).collect();
    session
        .available_model_catalog()
        .iter()
        .map(|m| ModelEntry {
            id: m.id.to_string(),
            name: m.name.clone(),
            provider: m.provider.to_string(),
            current: m.id.as_str() == current.model.as_str()
                && m.provider.as_str() == current.provider.as_str(),
            scoped: scoped.contains(m.id.as_str()),
        })
        .collect()
}

/// Resolve the external-editor command for the live session honoring Pi's precedence — settings
/// `externalEditor` → `$VISUAL` → `$EDITOR` → platform default (F14; Pi `getExternalEditorCommand`,
/// settings-manager.ts:846-848, consulted by `openExternalEditor` extension-editor.ts:117). Delegates
/// to the settings-tested [`cyrup_config::EffectiveSettings::external_editor`] (re-exported as
/// [`cyrup_session_svc::EffectiveSettings`]) so a configured editor is honored over the environment,
/// instead of the old inline `$VISUAL`/`$EDITOR`-only chain that ignored it.
fn resolve_external_editor(session: &AgentSession) -> String {
    session
        .services()
        .settings
        .effective()
        .external_editor(&cyrup_session_svc::EnvVars::from_process())
}

/// Spawn `editor_cmd path` (inheriting stdio) and, on a clean exit, return the file's contents with a
/// single trailing newline stripped (Pi's "reload the edited text"); `None` on a non-zero exit / spawn
/// failure (Pi's "no change"). `editor_cmd` is split on whitespace so `code --wait`-style commands work,
/// with `path` appended as the final argument. Pure (no terminal teardown, no `self`) so the resolved
/// command that actually runs can be exercised directly by a test — the terminal suspend/restore is the
/// caller's ([`App::edit_in_external_editor`]) responsibility.
fn run_editor_over_file(editor_cmd: &str, path: &std::path::Path) -> Option<String> {
    let mut parts = editor_cmd.split_whitespace();
    let status = parts
        .next()
        .map(|bin| std::process::Command::new(bin).args(parts).arg(path).status());
    if let Some(Ok(s)) = status
        && s.success()
        && let Ok(new_text) = std::fs::read_to_string(path)
    {
        let trimmed = new_text.strip_suffix('\n').unwrap_or(&new_text);
        return Some(trimmed.to_string());
    }
    None
}

fn tree_node_from_dag(n: &SessionDagNode) -> TreeNode {
    let kind = match n.kind {
        SessionDagKind::Message | SessionDagKind::Other => TreeKind::Message,
        SessionDagKind::Tool => TreeKind::ToolGroup,
        SessionDagKind::ModelChange => TreeKind::ModelChange,
        SessionDagKind::ThinkingChange => TreeKind::ThinkingChange,
        SessionDagKind::Compaction => TreeKind::Compaction,
    };
    TreeNode {
        id: n.entry_id.to_string(),
        depth: n.depth,
        label: truncate_summary(&n.label),
        kind,
        foldable: n.foldable,
        folded: false,
        has_label: n.has_label,
        time_label: n.is_leaf.then(|| "current".to_string()),
    }
}

/// Build the `/settings` grid rows from the live effective settings (Pi `settings-selector.ts`
/// `SettingsConfig` → `SettingItem`s, :479-712). Each row's `id` is the dotted settings key persisted
/// on cycle; toggles cycle `true`/`false`, choices cycle their fixed sets. Read straight off
/// [`cyrup_session_svc::EffectiveSettings`] so the displayed value matches the merged config.
fn settings_rows(eff: &cyrup_session_svc::EffectiveSettings, current_theme: &str) -> Vec<SettingRow> {
    let choices = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    vec![
        // The "Theme" row opens the theme picker (Pi `SettingItem.submenu` → `ThemeSubmenu`,
        // settings-selector.ts:603-610) — the one in-app path Pi reaches theme switching through.
        SettingRow::submenu("theme", "Theme", current_theme.to_string(), "theme"),
        SettingRow::toggle("compaction.enabled", "Auto-compact", eff.compaction_enabled()),
        SettingRow::toggle("terminal.showImages", "Show images", eff.show_images()),
        SettingRow::choice(
            "terminal.imageWidthCells",
            "Image width",
            eff.image_width_cells().to_string(),
            choices(&["60", "80", "120"]),
        ),
        SettingRow::toggle("images.autoResize", "Auto-resize images", eff.image_auto_resize()),
        SettingRow::toggle("images.blockImages", "Block images", eff.block_images()),
        SettingRow::toggle("enableSkillCommands", "Skill commands", eff.enable_skill_commands()),
        // `showHardwareCursor` / `terminal.clearOnShrink` — the effective getters need the env surface;
        // a default `EnvVars` yields the persisted setting (else `false`), which is what the grid edits.
        SettingRow::toggle(
            "showHardwareCursor",
            "Show hardware cursor",
            eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::default()),
        ),
        SettingRow::toggle(
            "terminal.clearOnShrink",
            "Clear on shrink",
            eff.clear_on_shrink(&cyrup_session_svc::EnvVars::default()),
        ),
        SettingRow::choice(
            "editorPaddingX",
            "Editor padding",
            eff.editor_padding_x().to_string(),
            choices(&["0", "1", "2", "3"]),
        ),
        // Inserted right after editor-padding, matching Pi (`settings-selector.ts:681-689` splices the
        // "Output padding" row after "editor-padding"). Cycles 0|1; honored live by the transcript.
        SettingRow::choice(
            "outputPad",
            "Output padding",
            eff.output_pad().to_string(),
            choices(&["0", "1"]),
        ),
        SettingRow::choice(
            "autocompleteMaxVisible",
            "Autocomplete max items",
            eff.autocomplete_max_visible().to_string(),
            choices(&["3", "5", "7", "10", "15", "20"]),
        ),
        // `httpIdleTimeoutMs` — cycle the raw millisecond presets (Pi shows human labels; the persisted
        // value is the same ms number). `disabled` = 0 (`HTTP_IDLE_TIMEOUT_CHOICES`, http-dispatcher.ts:5).
        SettingRow::choice(
            "httpIdleTimeoutMs",
            "HTTP idle timeout (ms)",
            eff.http_idle_timeout_ms().unwrap_or(300_000).to_string(),
            choices(&["30000", "60000", "120000", "300000", "0"]),
        ),
        SettingRow::toggle("hideThinkingBlock", "Hide thinking", eff.hide_thinking_block()),
        SettingRow::toggle("collapseChangelog", "Collapse changelog", eff.collapse_changelog()),
        SettingRow::toggle("quietStartup", "Quiet startup", eff.quiet_startup()),
        SettingRow::toggle(
            "enableInstallTelemetry",
            "Install telemetry",
            eff.enable_install_telemetry(),
        ),
        SettingRow::toggle(
            "terminal.showTerminalProgress",
            "Terminal progress",
            eff.show_terminal_progress(),
        ),
        SettingRow::choice(
            "steeringMode",
            "Steering mode",
            eff.steering_mode(),
            choices(&["all", "one-at-a-time"]),
        ),
        SettingRow::choice(
            "followUpMode",
            "Follow-up mode",
            eff.follow_up_mode(),
            choices(&["all", "one-at-a-time"]),
        ),
        SettingRow::choice(
            "transport",
            "Transport",
            eff.transport(),
            choices(&["auto", "websocket", "sse"]),
        ),
        SettingRow::choice(
            "doubleEscapeAction",
            "Double-escape action",
            eff.double_escape_action(),
            choices(&["fork", "tree", "none"]),
        ),
        SettingRow::choice(
            "treeFilterMode",
            "Tree filter mode",
            eff.tree_filter_mode(),
            choices(&["default", "no-tools", "user-only", "labeled-only", "all"]),
        ),
        SettingRow::choice(
            "defaultProjectTrust",
            "Default project trust",
            default_trust_label(eff.default_project_trust()),
            choices(&["ask", "always", "never"]),
        ),
    ]
}

/// The settings string for a [`cyrup_session_svc::DefaultProjectTrust`] (Pi serializes it as the
/// lowercase enum value `ask`/`always`/`never`).
fn default_trust_label(trust: cyrup_session_svc::DefaultProjectTrust) -> String {
    use cyrup_session_svc::DefaultProjectTrust as D;
    match trust {
        D::Ask => "ask",
        D::Always => "always",
        D::Never => "never",
    }
    .to_string()
}

/// Coerce a cycled `/settings` value string back into JSON for persistence: `true`/`false` → bool, an
/// integer → number, else a string (Pi's settings each have a typed `onChange`; the grid cycles the
/// display string, so we re-type it here).
fn parse_setting_value(value: &str) -> serde_json::Value {
    match value {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        other => match other.parse::<i64>() {
            Ok(n) => serde_json::Value::from(n),
            Err(_) => serde_json::Value::String(other.to_string()),
        },
    }
}

/// Format the saved trust-decision header line for the `/trust` selector (Pi `formatDecision`,
/// trust-selector.ts:23-31): `none`, or `trusted (path)` / `untrusted (path)`.
fn format_saved_trust(saved: &Option<cyrup_session_svc::TrustEntry>) -> String {
    match saved {
        None => "none".to_string(),
        Some(entry) => {
            let label = if entry.decision.is_trusted() { "trusted" } else { "untrusted" };
            format!("{label} ({})", entry.path.display())
        }
    }
}

/// The `/resume` row label for a session (Pi `session-selector.ts` row): its name (or first message),
/// trimmed to one line.
fn session_label(info: &cyrup_session_svc::SessionInfo) -> String {
    let raw = match &info.name {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ if !info.first_message.trim().is_empty() => info.first_message.clone(),
        _ => info.id.to_string(),
    };
    truncate_summary(&raw)
}

/// A monotonic recency key for a session's `modified` time (nanoseconds since the Unix epoch; `0`
/// before the epoch / on a clock fault). Drives the `Relevance` sort tie-break (newest first).
fn system_time_nanos(t: std::time::SystemTime) -> u128 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Render the `/arminsayshi` XBM bitmap as half-block art (`armin.ts`: 31×36, LSB-first, `1` =
/// background, `0` = foreground; two vertical pixels packed per cell into `█`/`▀`/`▄`/space). A pure,
/// deterministic transcript block (the animation effects are omitted as non-testable chrome).
fn armin_art() -> String {
    const WIDTH: usize = 31;
    const HEIGHT: usize = 36;
    const BYTES_PER_ROW: usize = WIDTH.div_ceil(8);
    const BITS: [u8; 144] = [
        255, 255, 255, 127, 255, 240, 255, 127, 255, 237, 255, 127, 255, 219, 255, 127, 255, 183,
        255, 127, 255, 119, 254, 127, 63, 248, 254, 127, 223, 255, 254, 127, 223, 63, 252, 127,
        159, 195, 251, 127, 111, 252, 244, 127, 247, 15, 247, 127, 247, 255, 247, 127, 247, 255,
        227, 127, 247, 7, 232, 127, 239, 248, 103, 112, 15, 255, 187, 111, 241, 0, 208, 91, 253,
        63, 236, 83, 193, 255, 239, 87, 159, 253, 238, 95, 159, 252, 174, 95, 31, 120, 172, 95, 63,
        0, 80, 108, 127, 0, 220, 119, 255, 192, 63, 120, 255, 1, 248, 127, 255, 3, 156, 120, 255,
        7, 140, 124, 255, 15, 206, 120, 255, 255, 207, 127, 255, 255, 207, 120, 255, 255, 223, 120,
        255, 255, 223, 125, 255, 255, 63, 126, 255, 255, 255, 127,
    ];
    // `1` (background) → false; `0` (foreground) → true. Out-of-range rows are background.
    let pixel = |x: usize, y: usize| -> bool {
        if y >= HEIGHT {
            return false;
        }
        let byte_index = y * BYTES_PER_ROW + x / 8;
        match BITS.get(byte_index) {
            Some(byte) => ((byte >> (x % 8)) & 1) == 0,
            None => false,
        }
    };
    let mut out = String::new();
    let rows = HEIGHT.div_ceil(2);
    for row in 0..rows {
        for x in 0..WIDTH {
            let upper = pixel(x, row * 2);
            let lower = pixel(x, row * 2 + 1);
            out.push(match (upper, lower) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
    }
    out
}

/// The six live-region row heights `[msg, band, images, slot, popup, footer]` for a viewport of
/// `avail` rows (audit #1). Filled by priority **from the bottom** — footer, then the editor/selector
/// slot (+completion popup), then the status band, then the inline image strip — and the message
/// region takes the remainder, **capped to the active turn's content height** so an empty turn never
/// balloons into a void (the old `Constraint::Min(1)` flex). The function is idempotent: feeding back
/// its own sum reproduces the same split, so [`render`] (called with the viewport height) and
/// [`live_region_height`] (called with the terminal height) never disagree on row counts.
fn region_constraints(state: &AppState, width: u16, avail: u16) -> [u16; 6] {
    let avail = avail.max(1);
    let max_editor = avail.saturating_sub(2).max(3);
    // A selector owns the slot at its desired height; otherwise the editor sizes to its line count +
    // the two rule rows (spec/tui/05 §1.1, spec/tui/03 §3.1).
    let want_slot = match state.selector.as_ref() {
        Some(active) => active.inner.desired_height(width).clamp(3, max_editor),
        // Size from the VISUAL (wrapped) line count at the same content width the editor renders at
        // (`view_width = area.width - 1`, one col reserved for the end-of-line cursor cell) so a long
        // or pasted single logical line grows the box one row per wrapped visual line instead of
        // clipping (Pi `editor.ts:1690`/`471`). The +2 is the two rule rows; the cap is unchanged.
        None => (state
            .editor
            .visual_line_count(width.saturating_sub(1) as usize)
            .min(u16::MAX as usize) as u16)
            .saturating_add(2)
            .clamp(3, max_editor),
    };
    // The completion popup is appended below the editor's bottom rule (spec/tui/04 §7); suppressed
    // while a selector owns the slot.
    let want_popup = if state.selector.is_some() {
        0
    } else {
        state.editor.autocomplete().map(|ac| ac.list.rendered_height()).unwrap_or(0)
    };
    let footer_max: u16 = if state.status.has_extension_statuses() { 3 } else { 2 };
    let want_status = state.indicator.is_active() || state.reserve_status_rows;
    let want_images: u16 = if state.selector.is_some() || state.pending_images.is_empty() {
        0
    } else {
        state
            .pending_images
            .iter()
            .map(|b| state.image_renderer.cell_size(b, width).1)
            .fold(0u16, |a, h| a.saturating_add(h))
    };

    let mut remaining = avail;
    let footer = footer_max.min(remaining).max(1);
    remaining = remaining.saturating_sub(footer);
    let slot = want_slot.min(remaining);
    remaining = remaining.saturating_sub(slot);
    let popup = want_popup.min(remaining);
    remaining = remaining.saturating_sub(popup);
    let band = if want_status { 2u16.min(remaining) } else { 0 };
    remaining = remaining.saturating_sub(band);
    let images = want_images.min(remaining);
    remaining = remaining.saturating_sub(images);
    // The message region = the active turn's content, plus the one startup-hint row at idle, capped
    // to whatever rows remain (so the inline viewport stays content-sized, not full-screen).
    let active = state.transcript.content_height(width as usize, &state.theme).min(u16::MAX as usize)
        as u16;
    let hint = u16::from(
        state.show_startup_hints && state.selector.is_none() && !state.transcript.has_active(),
    );
    let msg = active.max(hint).min(remaining);
    [msg, band, images, slot, popup, footer]
}

/// The inline-viewport height = the sum of the live-region rows (audit #1). Driven by
/// [`region_constraints`] against the **terminal** height so the content-sized viewport never
/// exceeds the screen.
fn live_region_height(state: &AppState, width: u16, term_height: u16) -> u16 {
    // A floating overlay (hotkeys/help; spec/tui/05 §2) is a modal that draws *over* the whole live
    // region — it needs the full screen to center its box, so the inline viewport expands to the
    // terminal height while one is open (the editor/footer still render behind it).
    if !state.overlays.is_empty() {
        return term_height.max(1);
    }
    region_constraints(state, width, term_height).iter().copied().fold(0u16, u16::saturating_add)
}

/// Pure render: lay out conversation / editor / status and render each component (`state -> frame`).
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let [msg_h, band_h, images_h, slot_h, popup_h, footer_h] =
        region_constraints(state, area.width, area.height);
    let _ = msg_h; // the message region absorbs the remainder via `Min(0)` below.
    let [msg_area, band_area, images_area, slot_area, popup_area, status_area] = Layout::vertical([
        // `Min(0)` (not the old `Min(1)`): the empty turn must not balloon the viewport (audit #1).
        Constraint::Min(0),
        Constraint::Length(band_h),
        Constraint::Length(images_h),
        Constraint::Length(slot_h),
        Constraint::Length(popup_h),
        Constraint::Length(footer_h),
    ])
    .areas(area);
    state.transcript.render(frame, msg_area, &state.theme);
    // The compact startup-help bar (`compactInstructions`, interactive-mode.ts:697-703) occupies the
    // bottom row of the otherwise-empty message area at startup — just above the editor — sourced from
    // the live keymap so rebinds reflect. It is suppressed once a submission lands (`show_startup_hints`
    // cleared) and while a selector owns the slot, so it never shifts the editor/footer geometry.
    if state.show_startup_hints
        && state.selector.is_none()
        && !state.transcript.has_active()
        && msg_area.height >= 1
    {
        let hint_row = ratatui::layout::Rect {
            x: msg_area.x,
            y: msg_area.y.saturating_add(msg_area.height - 1),
            width: msg_area.width,
            height: 1,
        };
        crate::chrome::render_compact_hints(frame, hint_row, &state.theme, &state.keymap);
    }
    if images_h > 0 {
        render_images(frame, images_area, state);
    }
    if band_h > 0 {
        let cancel = state.keymap.key_label(Action::Interrupt);
        state.indicator.render(frame, band_area, &state.theme, cancel.as_deref());
    }
    if let Some(active) = state.selector.as_mut() {
        active.inner.render(frame, slot_area, &state.theme);
    } else if let Some(loader) = state.loader.as_ref() {
        // A long inline op (e.g. `/share`'s gist creation) owns the slot with a `BorderedLoader`.
        loader.render(frame, slot_area, &state.theme, state.loader_tick);
    } else {
        state.editor.render(frame, slot_area, &state.theme);
        if let Some(ac) = state.editor.autocomplete() {
            let lines = ac.list.lines(popup_area.width, &state.theme);
            frame.render_widget(Paragraph::new(lines).style(state.theme.base_style()), popup_area);
        }
    }
    state.status.render(frame, status_area, &state.theme);
    // Floating overlays draw last, on top of the live region, bottom→top (spec/tui/05 §2; arch-10
    // §6.4): each clears its own `Rect` then renders its box.
    for overlay in state.overlays.iter_mut() {
        overlay.render(frame, area, &state.theme);
    }
}

/// Render the attached-image strip inline above the editor (`components/image.ts`): stack each
/// [`ImageBlock`] at its natural cell height, drawing the real protocol when `show_images` is on and a
/// text placeholder when off (spec/tui/06 §6). Honors the live image protocol negotiated at startup.
fn render_images(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    for block in &state.pending_images {
        if y >= bottom {
            break;
        }
        let want = state.image_renderer.cell_size(block, area.width).1.max(1);
        let h = want.min(bottom.saturating_sub(y));
        let cell = ratatui::layout::Rect { x: area.x, y, width: area.width, height: h };
        state.image_renderer.render(frame, cell, block, &state.theme, state.show_images);
        y = y.saturating_add(h);
    }
}

// ----------------------------------------------------------------- crossterm wiring ----

impl App<CrosstermBackend<Stdout>> {
    /// Build the production app: raw mode on, bracketed paste + Kitty keyboard flags enabled
    /// (best-effort, with graceful fallback, R-ARCH-TUI-008), inline viewport on stdout.
    pub fn into_stdout(theme: UiTheme) -> Result<Self, TuiError> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        out.execute(ratatui::crossterm::event::EnableBracketedPaste)?;
        // Kitty keyboard protocol where supported; ignore failure (legacy terminals).
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        App::new(CrosstermBackend::new(out), theme)
    }

    /// Draw one frame wrapped in synchronized-output markers (CSI 2026, R-10-002 / R-ARCH-TUI-004).
    pub fn draw_synchronized(&mut self) -> Result<(), TuiError> {
        let mut out = io::stdout();
        let _ = out.execute(BeginSynchronizedUpdate);
        let res = self.draw();
        let _ = out.execute(EndSynchronizedUpdate);
        res
    }

    /// Restore the terminal: pop keyboard flags, disable bracketed paste, leave raw mode, show
    /// cursor. Total and idempotent so a `Drop` guard / error path always leaves a usable terminal.
    pub fn restore(&mut self) -> Result<(), TuiError> {
        let mut out = io::stdout();
        let _ = execute!(out, PopKeyboardEnhancementFlags);
        let _ = out.execute(ratatui::crossterm::event::DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = self.terminal.show_cursor();
        Ok(())
    }

    /// Suspend the process to the background (Ctrl+Z / `app.suspend`, `core/keybindings.ts`): tear the
    /// terminal back down to a usable cooked state, raise `SIGTSTP` on our own process group so the
    /// shell regains control, then — when the user `fg`s us and the kernel delivers `SIGCONT` — restore
    /// raw mode + the inline viewport and redraw. The signal is raised by shelling out to `kill -s
    /// TSTP <pid>` so the crate stays `#![forbid(unsafe_code)]` with **no** new dependency (a libc
    /// `raise` would need an unsafe shim + a new dep; the `kill` path needs neither). Unix-only; on
    /// other platforms it degrades to a redraw.
    pub fn suspend(&mut self) -> Result<(), TuiError> {
        self.restore()?;
        #[cfg(unix)]
        {
            // Stop our own process group; `kill` exits before the stop takes effect, and we resume on
            // SIGCONT (shell `fg`) at the next statement.
            let pid = std::process::id().to_string();
            let _ = std::process::Command::new("kill").args(["-s", "TSTP", &pid]).status();
        }
        // Resumed (or non-unix): re-enter raw mode + flags, then redraw the live region.
        enable_raw_mode()?;
        let mut out = io::stdout();
        let _ = out.execute(ratatui::crossterm::event::EnableBracketedPaste);
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let _ = self.terminal.clear();
        self.draw_synchronized()
    }

    /// Open the editor buffer in an external editor (Ctrl+G / `app.editor.external`,
    /// `openExternalEditor` interactive-mode.ts:3611): run the caller-resolved `editor_cmd` (the
    /// settings `externalEditor` → `$VISUAL` → `$EDITOR` → default chain, see [`App::run`]), write the
    /// buffer to a temp `*.pi.md`, tear the TUI down to release the terminal, run the editor (inheriting
    /// stdio), and — on a clean exit — reload the edited text (trailing newline stripped). The terminal
    /// is always restored, even on error. No `unsafe`, no new dependency (`std::process` + `std::fs`).
    pub fn open_external_editor(&mut self, editor_cmd: &str) -> Result<(), TuiError> {
        let current = self.state.editor.text();
        if let Some(new_text) = self.edit_in_external_editor(&current, editor_cmd)? {
            self.state.editor.set_text(&new_text);
        }
        self.draw_synchronized()
    }

    /// `Ctrl+G` pressed inside the extension `ui.editor` dialog (L4 review §3;
    /// [`AppAction::OpenExternalEditorForSelector`]): seed `$VISUAL`/`$EDITOR` with the OPEN
    /// dialog's own buffer (never [`AppState::editor`], the live prompt draft — unrelated) and, on a
    /// clean exit, write the result back into the SAME dialog buffer via
    /// [`Selector::apply_external_edit`] — the dialog stays open (Pi never resolves it from this
    /// path, `extension-editor.ts:119-157`); only `Enter`/`Esc` close it. A no-op if no selector is
    /// open or the open one doesn't support external editing (`external_edit_text` returns `None`).
    fn open_external_editor_for_selector(&mut self, editor_cmd: &str) -> Result<(), TuiError> {
        let Some(current) = self.state.selector.as_ref().and_then(|a| a.inner.external_edit_text())
        else {
            return Ok(());
        };
        if let Some(new_text) = self.edit_in_external_editor(&current, editor_cmd)?
            && let Some(active) = self.state.selector.as_mut()
        {
            active.inner.apply_external_edit(&new_text);
        }
        self.draw_synchronized()
    }

    /// Run the resolved `editor_cmd` over `initial` text and return the edited result on a clean exit
    /// (`Ok(None)` on a non-zero exit / spawn failure / unwritable temp file — Pi's "no change"). Tears
    /// the TUI down for the duration and always restores it before returning, even on failure — the
    /// caller is left with a usable terminal either way.
    ///
    /// `editor_cmd` is resolved by the caller (`App::run`) through the SAME precedence Pi uses —
    /// settings `externalEditor` → `$VISUAL` → `$EDITOR` → platform default
    /// ([`cyrup_config::EffectiveSettings::external_editor`], settings-manager.ts:846,
    /// extension-editor.ts:117) — rather than the old inline `$VISUAL`/`$EDITOR`-only chain that
    /// silently ignored a configured `externalEditor` (F14). This method just SPAWNS the resolved
    /// command via [`run_editor_over_file`].
    ///
    /// The synchronous, TUI-suspending core both [`Self::open_external_editor`] (Ctrl+G on the live
    /// input buffer) and [`Self::open_external_editor_for_selector`] (Ctrl+G inside the extension
    /// `ui.editor` dialog, L4 review §3) share.
    ///
    /// This runs entirely synchronously on the caller's task (no `.await`) — reused directly inside
    /// `App::run`'s `select!` loop is safe (nothing here can deadlock against a concurrently-blocked
    /// guest, unlike the `execute_command`/`run_shortcut` paths, which must never await guest-reentrant
    /// work inline for exactly that reason; see `App::run`'s `AppAction::ExtensionShortcut` arm).
    fn edit_in_external_editor(
        &mut self,
        initial: &str,
        editor_cmd: &str,
    ) -> Result<Option<String>, TuiError> {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("cyrup-editor-{}.pi.md", std::process::id()));
        if std::fs::write(&tmp, initial).is_err() {
            self.state.transcript.push_status("external editor: could not write temp file");
            return Ok(None);
        }

        // Release the terminal (cooked mode, no inline viewport) so the editor owns the screen.
        self.restore()?;
        let result = run_editor_over_file(editor_cmd, &tmp);
        let _ = std::fs::remove_file(&tmp);

        // Re-enter raw mode + bracketed paste + Kitty flags; the caller redraws.
        enable_raw_mode()?;
        let mut out = io::stdout();
        let _ = out.execute(ratatui::crossterm::event::EnableBracketedPaste);
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let _ = self.terminal.clear();
        Ok(result)
    }

    /// The interactive event loop: `select!` over terminal input, the agent event stream, theme
    /// hot-reload, and cancellation (arch-10 §5). Renders with synchronized output. Submissions are
    /// routed to `session` (steer while streaming, else a fresh prompt; R-10-030).
    pub async fn run(
        &mut self,
        mut input: EventStream<InputEvent>,
        mut events: EventStream<AgentSessionEvent>,
        session: Arc<AgentSession>,
        runtime: Option<Arc<AgentSessionRuntime>>,
        mut theme_rx: Option<tokio::sync::watch::Receiver<Arc<ThemeData>>>,
        cancel: CancelToken,
    ) -> Result<(), TuiError> {
        // The active session + its event subscription are re-bound on every runtime replacement
        // (arch-11 §3.4): a session-swap command (or a runtime-side `SessionReplaced`) bumps the
        // runtime's generation `watch`, the loop drops the stale subscription, subscribes the new
        // session, and re-binds the UI ([`App::rebind_session`]). Without a runtime they are fixed.
        let mut session = session;
        let mut gen_rx = runtime.as_ref().map(|r| r.watch_generation());
        // The synchronous extension-dialog sink (L4 review §2.1): a loaded guest's `ui.{confirm,input,
        // select,editor}` capability blocks its OWN tokio task on a one-shot
        // (`LiveHostServices::ui_roundtrip`) while this loop's `ui_rx` arm renders the matching dialog
        // and replies once the user answers — the interactive-TUI mirror of `crates/cyrup-modes/src/
        // rpc.rs`'s `run_rpc`, which wires the SAME `UiSink` mechanism for RPC mode. Installed here
        // (only when a TUI is present — `App::run` is never invoked headless) and re-installed on every
        // session swap below, since a replacement session brings a fresh `LiveHostServices`.
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        session.services().host_services.set_ui_sink(ui_tx.clone());
        // Honor the persisted `outputPad` at boot (Pi seeds `this.outputPad = getOutputPad()`,
        // interactive-mode.ts:440): the transcript defaults to Pi's `1`, but a configured `0` must take
        // effect on the first frame. Re-read after each session swap below (a swap resets the transcript).
        self.state
            .transcript
            .set_output_pad(session.services().settings.effective().output_pad().max(0) as usize);
        // Same for `hideThinkingBlock` (Pi seeds `this.hideThinkingBlock = getHideThinkingBlock()`
        // before constructing any `AssistantMessageComponent`): the very first reasoning block must
        // already honour the persisted setting.
        self.state
            .transcript
            .set_hide_thinking_block(session.services().settings.effective().hide_thinking_block());
        // `terminal.showImages` / `terminal.imageWidthCells` govern how a tool result's `image`
        // content blocks render (TUI-007) — seed both before the first frame.
        let eff = session.services().settings.effective();
        self.state.show_images = eff.show_images();
        self.state.transcript.set_show_images(self.state.show_images);
        self.state
            .transcript
            .set_image_width_cells(eff.image_width_cells().clamp(1, u16::MAX as i64) as u16);
        self.draw_synchronized()?;
        // The spinner tick (spec/tui/01 §6.2 / §10): an 80 ms redraw used **only while** a status
        // indicator is active, so the Braille frame advances without a timer thread and an idle
        // session never busy-loops (the branch is `if`-gated on `indicator.is_active()`).
        let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
        spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The extension-UI dialog countdown tick (Pi's `CountdownTimer`, `countdown-timer.ts:21-30`):
        // a 1s redraw used **only while** an open `ui.{confirm,select,input}` dialog has a
        // guest-set `opts.timeout_ms` armed, so an idle session (or a dialog with no timeout) never
        // pays for it — mirrors the spinner's own `if`-gated pattern immediately above.
        let mut dialog_countdown = tokio::time::interval(Duration::from_secs(1));
        dialog_countdown.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // A live `!`/`!!` bash subprocess: its output receiver + a cancel token to kill it (`Esc`).
        // Kept as run-loop locals (not on `self`) so the `select!` borrow does not collide with the
        // input-arm `&mut self`.
        let mut bash_rx: Option<tokio::sync::mpsc::UnboundedReceiver<BashMsg>> = None;
        let mut bash_cancel: Option<CancelToken> = None;
        // A fired extension shortcut is spawned onto its own tokio task (see the
        // `AppAction::ExtensionShortcut` arm below for why); this channel carries its status/error
        // line back to the transcript once it settles, mirroring the `bash_rx` pattern above.
        let (shortcut_status_tx, mut shortcut_status_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        loop {
            let theme_changed = async {
                match theme_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            let bash_next = async {
                match bash_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            // Resolve to `true` when the runtime swaps the active session (generation bump). When no
            // runtime is threaded in, never resolves (single fixed session).
            let session_swapped = async {
                match gen_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = spinner.tick(), if self.state.indicator.is_active() => {
                    self.draw_synchronized()?;
                }
                _ = dialog_countdown.tick(),
                    if self.state.pending_ui_reply.as_ref().is_some_and(|p| p.deadline.is_some()) =>
                {
                    self.tick_extension_dialog_countdown();
                    self.draw_synchronized()?;
                }
                maybe_in = input.next() => {
                    let Some(ev) = maybe_in else { break };
                    match self.handle_input(&ev) {
                        AppAction::Quit => break,
                        AppAction::Suspend => self.suspend()?,
                        AppAction::OpenExternalEditor => {
                            let editor_cmd = resolve_external_editor(&session);
                            self.open_external_editor(&editor_cmd)?;
                        }
                        AppAction::OpenExternalEditorForSelector => {
                            let editor_cmd = resolve_external_editor(&session);
                            self.open_external_editor_for_selector(&editor_cmd)?;
                        }
                        AppAction::Interrupt => {
                            session.abort();
                            // Also kill a running bash child (the block was already marked cancelled
                            // in `apply_action`); the reader task's terminal `Done` clears `bash_rx`.
                            if let Some(c) = bash_cancel.take() {
                                c.cancel();
                            }
                        }
                        AppAction::InterruptRestoreQueued => {
                            // Pi `onEscape` while streaming (interactive-mode.ts:2636-2637):
                            // `restoreQueuedMessagesToEditor({abort: true})` — take-all BOTH queues,
                            // put their text back in the editor, and only then abort. Without the
                            // restore, an Esc during a turn silently discards every steering /
                            // follow-up message the user typed while it ran.
                            let (steering, follow_up) = session.drain_queue().await;
                            let queued: Vec<String> =
                                steering.into_iter().chain(follow_up).collect();
                            self.restore_queued_to_editor(&queued);
                            session.abort();
                            if let Some(c) = bash_cancel.take() {
                                c.cancel();
                            }
                        }
                        AppAction::RunBash { command, .. } => {
                            // Replace any prior job (its token is dropped → child orphaned-but-exits).
                            if let Some(c) = bash_cancel.take() {
                                c.cancel();
                            }
                            let (rx, c) = spawn_bash(command);
                            bash_rx = Some(rx);
                            bash_cancel = Some(c);
                        }
                        AppAction::Submit(text) => {
                            // Spawned, not awaited inline (L4 review §2.1 — the SAME deadlock reason
                            // as `ExtensionShortcut` below): `prompt_accepted`/`steer` run Pi's
                            // pre-send extension-command dispatch + `input`-hook fan-out INLINE, before
                            // the run itself is spawned (`session.rs` `prepare` →
                            // `try_execute_extension_command` / `emit_input_event`), and either can
                            // call a guest's synchronous `ui.*` capability — this is in fact the MOST
                            // common guest-reentrant path (an extension's own `/command` handler, or an
                            // `on_input` hook, prompting for confirmation). This arm never touches
                            // `self.state` — the optimistic transcript echo already happened
                            // synchronously in `dispatch_submission` — so no channel-back is needed.
                            let session = session.clone();
                            tokio::spawn(async move {
                                let ui = UserInput::text(text, InputSource::Tui);
                                if session.is_streaming().await {
                                    let _ = session.steer(ui).await;
                                } else {
                                    let _ = session.prompt_accepted(ui).await;
                                }
                            });
                        }
                        AppAction::FollowUp(text) => {
                            // Pi `handleFollowUp` (interactive-mode.ts:3554-3585): while a turn is
                            // streaming, queue the text as a follow-up (delivered once the agent goes
                            // idle — a SEPARATE queue from `steer`); when idle, Alt+Enter behaves like a
                            // plain Enter submit. The editor is cleared here (Pi's `setText("")` in both
                            // branches) since `apply_action` deferred the mutation until this async
                            // streaming check. Spawned, not awaited, for the same guest-reentrancy reason
                            // as `Submit`.
                            self.state.editor.clear();
                            let streaming = session.is_streaming().await;
                            if !streaming {
                                // Idle → behaves like a normal prompt: echo optimistically before send.
                                self.state.transcript.push_user(text.clone());
                            }
                            let session = session.clone();
                            tokio::spawn(async move {
                                let ui = UserInput::text(text, InputSource::Tui);
                                if streaming {
                                    let _ = session.follow_up(ui).await;
                                } else {
                                    let _ = session.prompt_accepted(ui).await;
                                }
                            });
                        }
                        AppAction::Dequeue => {
                            // Pi `handleDequeue` → `restoreQueuedMessagesToEditor`
                            // (interactive-mode.ts:3587-3594,3852-3871): drain BOTH the steering and
                            // follow-up queues (steering first, then follow-up — Pi's
                            // `[...steering, ...followUp]` order), join their text by blank lines, and
                            // prepend it to the current editor buffer. When nothing is queued, show
                            // Pi's exact `No queued messages to restore` status and leave the editor
                            // untouched.
                            // One atomic take-all (Pi's `clearAllQueues()` returns what it drained),
                            // not a read-then-clear pair — the split form loses any message queued
                            // between the two calls.
                            let (steering, follow_up) = session.drain_queue().await;
                            let queued: Vec<String> =
                                steering.into_iter().chain(follow_up).collect();
                            match self.restore_queued_to_editor(&queued) {
                                0 => self
                                    .state
                                    .transcript
                                    .push_status("No queued messages to restore"),
                                n => self.state.transcript.push_status(format!(
                                    "Restored {n} queued message{} to editor",
                                    if n > 1 { "s" } else { "" }
                                )),
                            }
                        }
                        AppAction::Command(cmd) => {
                            self.execute_command(cmd, &session, runtime.as_ref()).await;
                            if should_honor_extension_shutdown(&session, false) {
                                return Ok(());
                            }
                        }
                        AppAction::ExtensionShortcut(key) => {
                            // Route the fired shortcut to the owning live extension (R-08-017; Pi
                            // `registerShortcut` handler) — SPAWNED, not awaited inline (L4 review
                            // §2.1). The shortcut handler may itself call a synchronous
                            // `ui.{confirm,input,select,editor}` capability, which blocks ITS calling
                            // tokio task on `ui_roundtrip`'s one-shot reply until this very `select!`
                            // loop services `ui_rx` and answers it. Awaiting `run_shortcut` inline HERE
                            // would make that blocked task and the loop that must unblock it the SAME
                            // task — a single task's `poll()` can never reach a sibling `select!` arm
                            // while it is synchronously blocked deeper in its own call stack (tokio's
                            // `block_in_place` frees a WORKER THREAD for other tasks, not this task's
                            // own other branches) — a genuine self-deadlock. Spawning it as its own task
                            // keeps the main loop free to poll `ui_rx` concurrently, exactly why
                            // `SessionManager::spawn_run` already spawns agent-turn tool execution
                            // (session.rs `drive_run`) instead of awaiting it inline. A guest fault
                            // (or, now, a spawn-side error) is surfaced as a status block via
                            // `shortcut_status_tx`, never a panic; the run loop keeps going regardless.
                            let ext_host = session.services().ext_host.clone();
                            let shortcut_cancel = cancel.clone();
                            let status_tx = shortcut_status_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = ext_host.run_shortcut(&key, &shortcut_cancel).await {
                                    let _ = status_tx.send(format!("shortcut {key}: {e}"));
                                }
                            });
                        }
                        AppAction::Redraw | AppAction::None => {}
                    }
                    self.draw_synchronized()?;
                }
                Some(msg) = bash_next => {
                    match msg {
                        BashMsg::Chunk(chunk) => self.state.transcript.bash_append(&chunk),
                        BashMsg::Done(code) => {
                            // Feed `!` (not `!!`) output back into the session context, then commit
                            // the block to scrollback (`bash-execution.ts` → BashExecutionMessage).
                            if let Some(b) = self.state.transcript.bash() {
                                let excluded = b.excluded();
                                let command = b.command().to_string();
                                let output = b.output();
                                self.state.transcript.bash_complete(code, false);
                                self.state.transcript.commit_bash();
                                if !excluded && !output.trim().is_empty() {
                                    let payload = serde_json::json!({
                                        "command": command,
                                        "output": output,
                                        "exitCode": code,
                                    });
                                    let _ = session
                                        .append_custom_message("bashExecution", payload, false)
                                        .await;
                                }
                            }
                            bash_rx = None;
                            bash_cancel = None;
                        }
                    }
                    self.draw_synchronized()?;
                }
                Some(req) = ui_rx.recv() => {
                    // A loaded guest opened a `ui.*` dialog (L4 review §2.1). EVERY kind, including
                    // `editor` (L4 review §3 — an INLINE dialog is now the default, matching Pi's
                    // `ExtensionEditorComponent`; `$VISUAL`/`$EDITOR` is reachable only via the
                    // dialog's own `Ctrl+G`, `AppAction::OpenExternalEditorForSelector`, above), opens
                    // the matching input-slot selector via `open_extension_dialog` and waits for a
                    // future key event to confirm/cancel it (`AppState::pending_ui_reply`).
                    self.open_extension_dialog(req);
                    self.draw_synchronized()?;
                }
                Some(msg) = shortcut_status_rx.recv() => {
                    self.state.transcript.push_status(msg);
                    self.draw_synchronized()?;
                }
                maybe_ev = events.next() => {
                    let Some(ev) = maybe_ev else { continue };
                    // EXT-006: fold through the extension-aware path so a registered renderer
                    // actually draws the block (a custom message / a tool row). No renderer for the
                    // event's key ⇒ a sync pre-check short-circuits and this is the old behavior.
                    let ext_host = session.services().ext_host.clone();
                    self.ingest_event_with_extensions(&ev, &ext_host).await;
                    // SEAM-005 / EXT-005: a guest's `ctx.shutdown()` is honored at the settle point
                    // (Pi interactive-mode.ts:3137-3138 `case "agent_settled": await
                    // this.checkShutdownRequested()`), and only there — `agent_end` cannot tell us
                    // whether a retry or a queued continuation is still coming.
                    if should_honor_extension_shutdown(
                        &session,
                        matches!(ev, AgentSessionEvent::AgentSettled),
                    ) {
                        return Ok(());
                    }
                    self.draw_synchronized()?;
                }
                ok = theme_changed => {
                    if ok && let Some(rx) = theme_rx.as_ref() {
                        let data = rx.borrow().clone();
                        self.set_theme(UiTheme::from_theme_data(&data, 0));
                        self.draw_synchronized()?;
                    }
                }
                swapped = session_swapped => {
                    // A runtime replacement (a `/new`/`/resume`/`/fork`/`/reload`/`/import` op, or a
                    // runtime-side `SessionReplaced`, R-11-021) installed a new active session: drop
                    // the stale subscription, subscribe the NEW session's event stream, and re-bind
                    // the UI. Honors a runtime-driven swap identically to a UI-driven one.
                    if swapped && let Some(rt) = runtime.as_ref() {
                        let new_session = rt.session().await;
                        events = new_session.subscribe();
                        session = new_session;
                        self.rebind_session();
                        // The swapped-in session owns a fresh `LiveHostServices`; re-install the ui
                        // sink so a post-swap guest dialog still reaches this loop (L4 review §2.1,
                        // same re-install this run loop's `AppAction::Command` rebind mirrors from
                        // `crates/cyrup-modes/src/rpc.rs`'s `run_rpc`).
                        session.services().host_services.set_ui_sink(ui_tx.clone());
                        // `rebind_session` reset the transcript to Pi's default pad; re-read the
                        // swapped-in session's `outputPad` so a configured value survives the swap.
                        self.state.transcript.set_output_pad(
                            session.services().settings.effective().output_pad().max(0) as usize,
                        );
                        self.state.transcript.set_hide_thinking_block(
                            session.services().settings.effective().hide_thinking_block(),
                        );
                        let eff = session.services().settings.effective();
                        self.state.show_images = eff.show_images();
                        self.state.transcript.set_show_images(self.state.show_images);
                        self.state.transcript.set_image_width_cells(
                            eff.image_width_cells().clamp(1, u16::MAX as i64) as u16,
                        );
                                        // TUI-003: seed the view from the swapped-in session's conversation (Pi
                        // re-runs `renderInitialMessages()` after a tree/fork navigation,
                        // interactive-mode.ts:1737-1742). Without this a `/resume`, `/fork` or
                        // `/import` leaves the user staring at an empty transcript while the
                        // session file holds the whole history. `raw_context_messages` (NOT
                        // `messages()`) is Pi's `buildContextEntries()` projection: roles intact,
                        // so a compaction/branch summary, a `custom` message and a `!` run each
                        // reach their own component instead of replaying as user prose.
                        let restored = session.raw_context_messages().await;
                        if !restored.is_empty() {
                            self.replay_session(&restored);
                        }
                        // The swapped-in session owns a fresh extension host; re-source its
                        // registered shortcut key-ids (R-08-017) so a post-swap press still routes.
                        let shortcuts = session.services().ext_host.shortcut_keys();
                        self.state.set_extension_shortcuts(shortcuts);
                        self.draw_synchronized()?;
                    }
                }
            }
            if self.state.should_quit {
                break;
            }
        }
        self.restore()
    }
}

/// A streamed message from a running `!`/`!!` bash subprocess (`bash-execution.ts` output pump).
#[derive(Clone, Debug)]
enum BashMsg {
    /// A raw stdout/stderr chunk (merged; `appendOutput` strips ANSI + normalizes newlines).
    Chunk(String),
    /// The process exited (`setComplete`); carries the exit code (`None` if signalled/unknown).
    Done(Option<i32>),
}

/// Spawn `command` via `sh -c` (`bash-execution.ts` / the `!` handler), streaming merged
/// stdout+stderr chunks over the returned channel and a terminal [`BashMsg::Done`]. The returned
/// [`CancelToken`] kills the child when cancelled (`Esc` → `tui.select.cancel`). No `unsafe`.
fn spawn_bash(command: String) -> (tokio::sync::mpsc::UnboundedReceiver<BashMsg>, CancelToken) {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<BashMsg>();
    let cancel = CancelToken::new();
    let child_cancel = cancel.clone();
    tokio::spawn(async move {
        let spawned = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(BashMsg::Chunk(format!("{e}\n")));
                let _ = tx.send(BashMsg::Done(None));
                return;
            }
        };
        // Pump stdout + stderr concurrently into the same channel (merged like a terminal).
        async fn pump<R: tokio::io::AsyncRead + Unpin>(
            mut r: R,
            tx: tokio::sync::mpsc::UnboundedSender<BashMsg>,
        ) {
            let mut buf = [0u8; 4096];
            loop {
                match r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned();
                        if tx.send(BashMsg::Chunk(chunk)).is_err() {
                            break;
                        }
                    }
                }
            }
        }
        let out_task = child.stdout.take().map(|o| tokio::spawn(pump(o, tx.clone())));
        let err_task = child.stderr.take().map(|e| tokio::spawn(pump(e, tx.clone())));
        let status = tokio::select! {
            _ = child_cancel.cancelled() => {
                let _ = child.start_kill();
                child.wait().await.ok()
            }
            s = child.wait() => s.ok(),
        };
        if let Some(t) = out_task {
            let _ = t.await;
        }
        if let Some(t) = err_task {
            let _ = t.await;
        }
        let _ = tx.send(BashMsg::Done(status.and_then(|s| s.code())));
    });
    (rx, cancel)
}

/// A terminal input stream backed by a blocking `event::read()` reader thread (the async crossterm
/// `EventStream` feature is not enabled in this build; arch-10 §5 fallback). Maps `crossterm::Event`
/// to [`InputEvent`] and forwards over an unbounded channel; stops when `cancel` fires.
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    std::thread::spawn(move || {
        while !cancel.is_cancelled() {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if let Some(mapped) = map_event(ev)
                            && tx.send(mapped).is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
    Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

/// Map a crossterm event to our [`InputEvent`] (filtering non-press key kinds).
fn map_event(ev: Event) -> Option<InputEvent> {
    match ev {
        Event::Key(k) if !matches!(k.kind, KeyEventKind::Release) => Some(InputEvent::Key(k)),
        Event::Key(_) => None,
        Event::Paste(s) => Some(InputEvent::Paste(s)),
        Event::Resize(w, h) => Some(InputEvent::Resize(w, h)),
        Event::FocusGained => Some(InputEvent::FocusGained),
        Event::FocusLost => Some(InputEvent::FocusLost),
        Event::Mouse(_) => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod clipboard_paste_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Pi's clipboard paste inserts the materialized temp-file PATH into the editor as ordinary text
    /// (`this.editor.insertTextAtCursor(filePath)`, interactive-mode.ts:2552) — NOT an inline image.
    /// This drives the exact `insert_clipboard_image_path` step the Ctrl+V handler calls, then the
    /// real Enter-submit path, then the `UserInput` the run loop builds from that text — proving the
    /// OUTGOING message the LLM receives is text carrying the path with NO image content block
    /// (`AppAction::Submit` → `UserInput::text`, app.rs:3158; Pi `userContent = [{type:text}]` +
    /// (empty) images, agent-session.ts:1117).
    #[test]
    fn clipboard_paste_inserts_path_as_text_with_no_image_block() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

        // The path a clipboard image is materialized to (mirrors the real `cyrup-clipboard-<uuid>.png`
        // under the OS temp dir; it need not exist on disk — insertion is a pure text edit). On macOS
        // this is `/var/folders/…/T/…`, a leading-slash path — the case that must NOT be mistaken for
        // a slash command on submit.
        let path = std::env::temp_dir().join("cyrup-clipboard-0198f000-test.png");
        let path_str = path.to_string_lossy().to_string();

        app.insert_clipboard_image_path(&path);

        // Pi mechanism: the bare path is now editable text in the buffer …
        assert_eq!(app.state().editor.text(), path_str, "path must land in the editor as text");
        // … and is NOT embedded as an inline image (the former `pending_images` embed is gone), so the
        // potentially-huge raster never floods context.
        assert!(
            app.pending_images().is_empty(),
            "clipboard paste must not embed an image block into pending_images"
        );

        // Real submit path: plain Enter routes editor text → `dispatch_submission` → `AppAction::Submit`
        // (a leading-slash temp path fuzzy-matches no command, so it dispatches as a text prompt).
        let enter = InputEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let action = app.handle_input(&enter);
        assert_eq!(
            action,
            AppAction::Submit(path_str.clone()),
            "the pasted path must submit as a text prompt, not a slash command"
        );

        // The run loop turns that submitted text into the outgoing user message via `UserInput::text`
        // (app.rs:3158): the LLM receives the path AS TEXT with an EMPTY image set. `into_agent_message`
        // then yields a single text content block (Pi's `[{type:text,text}]` + no images).
        let outgoing = UserInput::text(path_str.clone(), InputSource::Tui);
        assert_eq!(outgoing.text, path_str, "outgoing message text is the pasted path");
        assert!(
            outgoing.images.is_empty(),
            "outgoing user message must carry no image content block"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod ctrl_c_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn ctrl_c() -> InputEvent {
        InputEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
    }

    /// F10 — Pi `handleCtrlC` (interactive-mode.ts:3361-3369): a second Ctrl+C within 500 ms exits,
    /// with NO emptiness gate (the first press clears the editor and records the time even when the
    /// buffer is non-empty; only the timing — not emptiness — gates the exit).
    #[test]
    fn double_ctrl_c_within_500ms_exits_regardless_of_editor_contents() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        // A NON-empty editor: the first Ctrl+C still only clears (no exit) — disproving the old
        // "exit when already empty" mis-statement by showing the inverse: contents don't force a clear
        // vs exit choice, timing does.
        app.editor_mut().set_text("some draft text");
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Redraw, "first Ctrl+C clears, never exits");
        assert_eq!(app.state().editor.text(), "", "first Ctrl+C cleared the buffer");
        assert!(!app.state().should_quit, "one press must not exit");
        // Immediate second press (well within 500 ms) → exit.
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Quit, "second Ctrl+C within 500 ms exits");
        assert!(app.state().should_quit, "double-tap sets the quit flag");
    }

    /// A lone Ctrl+C on an EMPTY editor does NOT exit (there is no emptiness gate), and a press that
    /// lands MORE than 500 ms after the previous one re-clears + re-arms rather than exiting.
    #[test]
    fn single_or_stale_ctrl_c_does_not_exit() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        // Empty editor, single press → clear/record, NOT exit (the mis-stated rule would have exited).
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Redraw, "empty single Ctrl+C must not exit");
        assert!(!app.state().should_quit);
        // Age the recorded press beyond the 500 ms window; the next press is a fresh first tap.
        app.state_mut().last_sigint =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(600));
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Redraw, "a >500 ms-later Ctrl+C re-arms");
        assert!(!app.state().should_quit, "outside the window is not a double-tap");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod live_floor_tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// The FLICKER fix's height logic (a unit guard — the definitive check is the pty drive): while a
    /// turn is active the live-region height uses a grow-only floor so it does NOT track per-tool
    /// content churn (which is what forced a `resize_viewport`/`reanchor_inline` reconstruction — the
    /// flicker source — on essentially every tool event). The instant the turn goes idle the floor
    /// resets so the region collapses back to the compact editor/footer (void-fix).
    #[test]
    fn live_floor_grows_then_holds_during_a_turn_and_resets_when_idle() {
        let mut app = App::new(TestBackend::new(80, 30), UiTheme::dark()).unwrap();
        app.status_mut().set_model("anthropic/claude-opus-4-8");
        app.draw().unwrap();
        let idle = app.viewport_height();

        // Turn goes active (AgentStart sets `status.streaming`); a burst of finished tools grows the
        // live tail before it is committed.
        app.status_mut().set_streaming(true);
        for i in 0..8u32 {
            let name = format!("read_{i}");
            app.transcript_mut()
                .push_tool_start(name.clone(), serde_json::json!({ "path": format!("file_{i}.md") }));
            app.transcript_mut().push_tool_end(
                name,
                false,
                Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("body {i}") }] })),
            );
        }
        app.draw().unwrap();
        let grown = app.viewport_height();
        assert!(grown > idle, "viewport should grow for the live tool tail ({grown} vs idle {idle})");

        // The finished tools commit to native scrollback mid-turn (SCREEN-FILL fix): the live content
        // collapses, but the floor HOLDS the viewport height so no reconstruction is triggered.
        app.transcript_mut().commit_finished_leading_tools();
        assert_eq!(app.state().transcript.active_tools().len(), 0, "finished tools left the tail");
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            grown,
            "grow-only floor must hold the height across the mid-turn commit (no per-tool reconstruct)"
        );

        // Turn goes idle (AgentEnd clears `status.streaming`): the floor resets and the region
        // collapses back to the compact idle height (void-fix preserved).
        app.status_mut().set_streaming(false);
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            idle,
            "idle viewport must collapse back to the compact region after the turn"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod external_editor_tests {
    use super::*;

    /// F14: the RESOLVED editor command is exactly what runs over the temp file — proving
    /// `edit_in_external_editor` spawns the command it is handed (which `App::run` resolves via
    /// `resolve_external_editor` → `EffectiveSettings::external_editor`, honoring settings
    /// `externalEditor` over `$VISUAL`/`$EDITOR`) rather than an inline env-only chain. A no-arg
    /// executable script (so `split_whitespace` yields just the script path + the appended file arg)
    /// rewrites the file; the reloaded text is the script's output.
    #[test]
    #[cfg(unix)]
    fn resolved_editor_command_is_the_one_that_runs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-editor.sh");
        std::fs::write(&script, "#!/bin/sh\nprintf 'REWRITTEN BY EDITOR' > \"$1\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "original text").unwrap();

        let out = run_editor_over_file(script.to_str().unwrap(), &file);
        assert_eq!(
            out.as_deref(),
            Some("REWRITTEN BY EDITOR"),
            "the resolved editor's edit is reloaded"
        );
    }

    /// A non-zero editor exit yields `None` — Pi's "no change" (`false` exits 1 without editing).
    #[test]
    #[cfg(unix)]
    fn nonzero_editor_exit_is_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "keep me").unwrap();
        assert_eq!(run_editor_over_file("false", &file), None);
    }

    /// A trailing newline the editor leaves is stripped once (Pi's `.replace(/\n$/, "")`).
    #[test]
    #[cfg(unix)]
    fn trailing_newline_is_stripped_once() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("nl-editor.sh");
        std::fs::write(&script, "#!/bin/sh\nprintf 'line one\\n' > \"$1\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(run_editor_over_file(script.to_str().unwrap(), &file).as_deref(), Some("line one"));
    }
}

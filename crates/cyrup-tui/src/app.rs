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

use cyrup_core::{CancelToken, EventStream};
use cyrup_provider::StreamEvent;
use cyrup_resources::theme::ThemeData;
use cyrup_session_svc::{AgentSession, AgentSessionEvent, InputSource, UserInput};
use cyrup_session_svc::{ForkPosition, NavigateTreeOptions};
use futures::StreamExt;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
};
use ratatui::crossterm::{execute, ExecutableCommand};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::commands::{CommandRegistry, Dispatch};
use crate::component::{Component, InputEvent};
use crate::editor::{EditorOutcome, InputEditor};
use crate::error::TuiError;
use crate::image::{ImageBlock, ImageRenderer};
use crate::keymap::{Action, EditorAction, Keymap, SelectKeymap, TreeKeymap};
use crate::overlay::{HotkeyRow, HotkeysOverlay, Overlay, OverlayOutcome};
use crate::selector::{
    CheckboxSelector, ListSelector, Selector, SelectorKind, SelectorOutcome,
};
use crate::session_selector::{SessionRow, SessionSelector, SessionSelectorOutcome};
use crate::settings_selector::{SettingRow, SettingsSelector, TrustSelector};
use crate::status::StatusLine;
use crate::status_indicator::{IndicatorKind, StatusIndicator, SPINNER_INTERVAL};
use crate::theme::UiTheme;
use crate::transcript::{content_text, entry_lines, TranscriptView};
use crate::tree_selector::{TreeNode, TreeSelector};

/// The number of visual lines a `PageUp`/`PageDown` scrolls the active region by (a conservative
/// screenful; spec/tui/07 page-scroll). Resolved on the pure input thread without the live viewport
/// height, then clamped against the real content at render time.
const PAGE_SCROLL_LINES: usize = 10;

/// The decision produced by feeding one input event to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    /// The user submitted a prompt (already optimistically shown in the transcript).
    Submit(String),
    /// The user requested an abort/interrupt of the in-flight run (Esc).
    Interrupt,
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
    /// A recognized slash command whose effect lives at the session/data layer (`setupEditorSubmitHandler`,
    /// interactive-mode.ts:2549-2734). The run loop executes it against the [`AgentSession`] (open a
    /// data-bound selector after sourcing its rows, drive the session lifecycle, export, copy, …).
    Command(AppCommand),
    /// State changed; the frame should be redrawn.
    Redraw,
    /// Nothing to do.
    None,
}

/// A slash command whose execution the run loop performs against the session/resources layer (the
/// in-crate effects — `/hotkeys`, `/debug`, `/changelog`, `/quit`, the 3 dependency-free selectors —
/// are applied directly in [`App::dispatch_submission`] and never become an `AppCommand`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppCommand {
    /// Open a data-bound selector; the run loop sources its rows from session-svc / resources.
    OpenSelector(SelectorKind),
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
    /// Committed lines already emitted to native scrollback via `Terminal::insert_before`
    /// (R-ARCH-TUI-003). Kept as a test-visible accumulator mirroring exactly what was handed to
    /// `insert_before`; never re-rendered inside the inline viewport.
    pub scrollback: Vec<Line<'static>>,
}

impl AppState {
    /// Fresh state with the given theme.
    pub fn new(theme: UiTheme) -> Self {
        AppState {
            transcript: TranscriptView::new(),
            editor: InputEditor::new(),
            status: StatusLine::default(),
            indicator: StatusIndicator::new(),
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
            scrollback: Vec::new(),
        }
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

/// The interactive front-end over an injectable backend.
pub struct App<B: Backend> {
    terminal: Terminal<B>,
    state: AppState,
}

impl<B: Backend> App<B> {
    /// Build an app over `backend` using an **inline viewport** sized to the backend height
    /// (R-ARCH-TUI-003). No alternate screen is entered.
    pub fn new(backend: B, theme: UiTheme) -> Result<Self, TuiError> {
        let size = backend.size().map_err(|e| TuiError::Backend(e.to_string()))?;
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(size.height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(App { terminal, state: AppState::new(theme) })
    }

    /// Immutable state access.
    pub fn state(&self) -> &AppState {
        &self.state
    }
    /// Mutable state access (drive the transcript/editor/status directly).
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
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

    /// Clear all attached images (after the prompt is sent, or on `Esc`).
    pub fn clear_images(&mut self) {
        self.state.pending_images.clear();
    }

    /// The images attached to the next prompt (test/inspection access).
    pub fn pending_images(&self) -> &[ImageBlock] {
        &self.state.pending_images
    }

    /// Probe the controlling TTY for its real image protocol (Kitty/iTerm2/sixel), upgrading from the
    /// portable half-block default (`terminal-image.ts` capability handshake). Called by the binary at
    /// startup; tests keep the half-block default so the inline path renders to `TestBackend`.
    pub fn detect_image_support(&mut self) {
        self.state.image_renderer = ImageRenderer::detect();
    }

    /// Apply a new theme, bumping its generation so caches invalidate (R-10-026).
    pub fn set_theme(&mut self, mut theme: UiTheme) {
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// Render one frame: first flush newly-committed entries to native scrollback (R-ARCH-TUI-003),
    /// then draw the active region into the inline viewport (pure: `state -> frame`).
    pub fn draw(&mut self) -> Result<(), TuiError> {
        self.flush_committed()?;
        let App { terminal, state } = self;
        terminal
            .draw(|frame| render(frame, state))
            .map_err(|e| TuiError::Backend(e.to_string()))?;
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
        let lines: Vec<Line<'static>> = committed
            .iter()
            .flat_map(|e| entry_lines(e, &self.state.theme, width))
            .collect();
        self.state.scrollback.extend(lines.iter().cloned());
        let style = self.state.theme.base_style();
        let height = lines.len().min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(lines).style(style).render(buf.area, buf);
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
                if let Some(action) = self.state.keymap.action_for(key) {
                    return self.apply_action(action);
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
    /// In-crate effects (the 3 dependency-free selectors, info blocks, quit) are applied here directly
    /// and return [`AppAction::Redraw`]; session/data-bound effects return [`AppAction::Command`] for
    /// the run loop to execute against the [`AgentSession`].
    fn run_command(&mut self, name: &str, arg: Option<String>) -> AppAction {
        use AppCommand as C;
        let cmd = |c| AppAction::Command(c);
        match name {
            // --- in-crate selectors (dependency-free) ---
            "think" => {
                self.open_selector(SelectorKind::Thinking);
                AppAction::Redraw
            }
            "theme" => {
                self.open_selector(SelectorKind::Theme);
                AppAction::Redraw
            }
            "show-images" => {
                self.open_selector(SelectorKind::ShowImages);
                AppAction::Redraw
            }
            // --- data-bound selectors (run loop sources rows) ---
            "model" => cmd(C::OpenSelector(SelectorKind::Model)),
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
                self.state.transcript.discard_streaming();
                self.state.transcript.commit_tools();
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                AppAction::Interrupt
            }
            // `app.clear` (Ctrl+C): clear the editor buffer; if it was already empty Pi treats a
            // second press as exit (double-Ctrl+C). Here a clear is always a redraw.
            Action::Clear => {
                self.state.editor.clear();
                AppAction::Redraw
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
                self.close_selector(true);
                AppAction::Redraw
            }
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
                self.state.transcript.push_status(format!("thinking → {value}"));
                None
            }
            SelectorKind::ShowImages => {
                self.state.show_images = value == "yes";
                let label = if self.state.show_images { "inline" } else { "placeholder" };
                self.state.transcript.push_status(format!("images → {label}"));
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
    pub async fn execute_command(&mut self, cmd: AppCommand, session: &Arc<AgentSession>) {
        use AppCommand as C;
        match cmd {
            C::OpenSelector(SelectorKind::Model) => {
                let current = session.model().model.to_string();
                let rows: Vec<(String, String, Option<String>)> = session
                    .scoped_models()
                    .into_iter()
                    .map(|sm| {
                        let id = sm.model.id.to_string();
                        (id.clone(), sm.model.name.clone(), Some(sm.model.provider.to_string()))
                    })
                    .collect();
                let selected = rows.iter().position(|(v, _, _)| *v == current).unwrap_or(0);
                if rows.is_empty() {
                    self.state.transcript.push_status("no models available (configure providers)");
                } else {
                    self.open_data_selector(SelectorKind::Model, rows, selected);
                }
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
                // `/tree` (tree-selector.ts): the session DAG flattened into [`TreeNode`]s. The live DAG
                // getter is the one L5 residual (residual ledger), so the navigable node set is sourced
                // from the fork anchors — the user-message spine of the conversation — each a depth-0
                // `Message` node whose confirm drives `navigate_tree`. The connector/fold/filter engine
                // (the bulk of the 47KB component) is already built in `tree_selector.rs`.
                let anchors = session.user_messages_for_forking().await;
                if anchors.is_empty() {
                    self.state.transcript.push_status("no session history to navigate");
                } else {
                    let nodes: Vec<TreeNode> = anchors
                        .iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let mut node = TreeNode::message(
                                a.entry_id.to_string(),
                                0,
                                truncate_summary(&a.text),
                            );
                            node.time_label = Some(format!("#{}", i + 1));
                            node
                        })
                        .collect();
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
                let rows = settings_rows(session.services().settings.effective());
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
                let entry = cyrup_core::EntryId::from(value.as_str());
                match session.fork_at_entry(&entry, ForkPosition::Before).await {
                    Ok(_) => self.state.transcript.push_status("forked from message"),
                    Err(e) => self.state.transcript.push_status(format!("fork error: {e}")),
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
                // The chosen session file path (the runtime swap is the L7 residual, gap #3).
                self.state.transcript.push_status(format!("resume {value} (/reload to switch)"));
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
                match session.persist_setting(cyrup_session_svc::SettingsScope::Global, &id, json) {
                    Ok(()) => self.state.transcript.push_status(format!("{id} → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("settings error: {e}")),
                }
            }
            C::Compact(arg) => match session.compact(arg).await {
                Ok(_) => self.state.transcript.push_status("compacted context"),
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
            // Session-lifecycle ops (`/new`, `/import`, `/reload`) drive the L7 `SessionRuntime`
            // (`new_session`/`import_from_jsonl`/`reload`); threading the runtime into the run loop +
            // re-subscribing on the generation bump is residual gap #3 (the run loop holds a fixed
            // `Arc<AgentSession>`). Surface the request so the path is real (no silent drop).
            C::NewSession => self.state.transcript.push_status("starting new session…"),
            C::Reload => self.state.transcript.push_status("reloading resources…"),
            C::Import(p) => self
                .state
                .transcript
                .push_status(format!("importing session {}", p.unwrap_or_default())),
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
        match ev {
            AgentSessionEvent::AgentStart => {
                self.state.status.set_streaming(true);
                self.state.indicator.working();
            }
            AgentSessionEvent::AgentEnd { .. } => {
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                self.state.transcript.commit_assistant(None);
                // Commit the turn's live tool executions into scrollback (`tool-execution.ts` tools
                // persist through the turn, then scroll up as committed history).
                self.state.transcript.commit_tools();
            }
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
                    self.state.transcript.push_custom_message(kind, body);
                }
            }
            AgentSessionEvent::MessageUpdate { assistant_message_event, .. } => {
                self.ingest_stream_event(assistant_message_event);
            }
            AgentSessionEvent::ToolExecutionStart { tool_name, args, .. } => {
                self.state.transcript.push_tool_start(tool_name.clone(), tool_args_summary(args));
            }
            AgentSessionEvent::ToolExecutionUpdate { partial_result, .. } => {
                self.state.transcript.push_tool_update(tool_result_text(partial_result));
            }
            AgentSessionEvent::ToolExecutionEnd { tool_name, is_error, result, .. } => {
                self.state.transcript.push_tool_end(
                    tool_name.clone(),
                    *is_error,
                    tool_result_text(result),
                );
            }
            AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                self.state.status.set_queued(steering.len().saturating_add(follow_up.len()));
            }
            AgentSessionEvent::CompactionStart { .. } => {
                self.state.indicator.set(IndicatorKind::Compaction, None);
                self.state.transcript.push_status("compacting context…");
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
            AgentSessionEvent::AutoRetryStart { attempt, max_attempts, .. } => {
                self.state
                    .indicator
                    .set(IndicatorKind::Retry, Some(format!("Retrying ({attempt}/{max_attempts})…")));
                self.state.transcript.push_status(format!("retrying ({attempt}/{max_attempts})…"));
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
                // Mirror the level into the footer right cluster (`• {level}`, footer.ts:186-188).
                self.state.status.set_thinking_level(level.clone());
                self.state.transcript.push_status(format!("thinking → {level}"));
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
    /// `AssistantMessage` text and records its token usage in the footer. Non-text streaming frames
    /// (start/text-start/text-end/thinking*/toolcall*) carry only the running `partial`; the
    /// authoritative text reaches us via `TextDelta` + the terminal, so nothing is rendered for them.
    fn ingest_stream_event(&mut self, ev: &StreamEvent) {
        match ev {
            StreamEvent::TextDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_assistant_delta(delta);
                }
            }
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. } => {
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
            }
            _ => {}
        }
    }
}

/// Flatten a styled [`Line`] into its plain text (concatenated span content).
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Derive a one-line argument summary for a tool call from its JSON args (`renderCall` summary,
/// `tool-execution.ts`). Prefers the conventional positional keys (`file_path`/`path`/`command`/
/// `pattern`/`url`/`query`); falls back to the first string value, else a compact JSON of scalars.
fn tool_args_summary(args: &serde_json::Value) -> Option<String> {
    let obj = args.as_object()?;
    for key in ["file_path", "path", "command", "pattern", "url", "query", "prompt", "name"] {
        if let Some(v) = obj.get(key).and_then(|v| v.as_str())
            && !v.is_empty()
        {
            return Some(truncate_summary(v));
        }
    }
    // First string-valued field, else nothing (avoid dumping nested objects).
    obj.values()
        .find_map(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(truncate_summary)
}

/// Extract human-readable text from a tool result JSON value (`getTextOutput`,
/// `core/tools/render-utils.ts`): a string is used verbatim; an object's `text`/`output`/`content`
/// string field is preferred; an array of `{text}` blocks is joined; otherwise `None`.
fn tool_result_text(result: &serde_json::Value) -> Option<String> {
    fn from_value(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => {
                for key in ["text", "output", "stdout", "content", "message"] {
                    if let Some(s) = o.get(key).and_then(|x| x.as_str()) {
                        return Some(s.to_string());
                    }
                }
                None
            }
            serde_json::Value::Array(items) => {
                let joined: Vec<String> = items.iter().filter_map(from_value).collect();
                (!joined.is_empty()).then(|| joined.join("\n"))
            }
            _ => None,
        }
    }
    from_value(result).filter(|s| !s.trim().is_empty())
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

/// Truncate a one-line summary to a sane length (avoid overrunning the marker line).
/// Detect a `Custom`-role [`cyrup_agent::AgentMessage`] from its serde projection and return its
/// `(kind, body)` for [`TranscriptView::push_custom_message`](crate::transcript::TranscriptView::push_custom_message).
/// `AgentMessage` is only a dev-dependency here, so the message is inspected through `serde_json`
/// (`{"role":"custom","kind":…,"payload":…}`) instead of a direct pattern match — no dep ripple.
/// Returns `None` for any non-custom (core user/assistant/toolResult) message.
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

/// Build the `/settings` grid rows from the live effective settings (Pi `settings-selector.ts`
/// `SettingsConfig` → `SettingItem`s, :479-712). Each row's `id` is the dotted settings key persisted
/// on cycle; toggles cycle `true`/`false`, choices cycle their fixed sets. Read straight off
/// [`cyrup_session_svc::EffectiveSettings`] so the displayed value matches the merged config.
fn settings_rows(eff: &cyrup_session_svc::EffectiveSettings) -> Vec<SettingRow> {
    let choices = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    vec![
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

/// Pure render: lay out conversation / editor / status and render each component (`state -> frame`).
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let max_editor = area.height.saturating_sub(2).max(3);
    // A selector occupies the editor slot at its desired (dynamic) height; otherwise the editor sizes
    // to its line count + the two rule rows (spec/tui/05 §1.1: the selector "grows the live region").
    let slot_h = match state.selector.as_ref() {
        Some(active) => active.inner.desired_height(area.width).clamp(3, max_editor),
        None => {
            let editor_rows =
                (state.editor.line_count().min(u16::MAX as usize) as u16).saturating_add(2);
            editor_rows.clamp(3, max_editor)
        }
    };
    // The autocomplete popup is appended directly below the editor's bottom rule, inside the live
    // region (spec/tui/04 §7) — not a floating overlay. Suppressed while a selector owns the slot.
    let popup_h = if state.selector.is_some() {
        0
    } else {
        state.editor.autocomplete().map(|ac| ac.list.rendered_height()).unwrap_or(0)
    };
    // The footer is two rows (location + usage/model) per spec/tui/01 §4; it collapses to one when
    // the viewport is too short to spare the second row.
    let chrome_h = slot_h.saturating_add(popup_h);
    // The footer is up to three rows (location · usage/model · extension statuses, spec/tui/01 §4 +
    // footer.ts:232-241). The third row only appears when an extension published a status; each row
    // is dropped when the viewport is too short to spare it (always keeping ≥1 message row).
    let footer_max: u16 = if state.status.has_extension_statuses() { 3 } else { 2 };
    let footer_h = if area.height >= chrome_h.saturating_add(footer_max.saturating_add(1)) {
        footer_max
    } else if area.height >= chrome_h.saturating_add(3) {
        2
    } else {
        1
    };
    // The working/idle status band (spec/tui/01 §6) is a 2-row region between the active turn and the
    // editor — reserved only when an indicator is active AND there is comfortable room, so an idle
    // viewport (and the short footer-only test layouts) keep their exact prior geometry (§6.3: Pi's
    // non-`clearOnShrink` default is 0 idle rows). `reserve_status_rows` forces the 2 rows always.
    let want_status = state.indicator.is_active() || state.reserve_status_rows;
    let room = area.height >= chrome_h.saturating_add(footer_h).saturating_add(3);
    let band_h: u16 = if want_status && room { 2 } else { 0 };
    // Attached images render inline above the editor (`components/image.ts`): each block reserves its
    // natural cell height (clamped so they never crowd out the message/editor rows). The strip is
    // suppressed while a selector owns the slot (the editor is hidden then).
    let images_h: u16 = if state.selector.is_some() || state.pending_images.is_empty() {
        0
    } else {
        let budget = area
            .height
            .saturating_sub(chrome_h.saturating_add(footer_h).saturating_add(band_h).saturating_add(1));
        let natural: u16 = state
            .pending_images
            .iter()
            .map(|b| state.image_renderer.cell_size(b, area.width).1)
            .fold(0u16, |a, h| a.saturating_add(h));
        natural.min(budget)
    };
    let [msg_area, band_area, images_area, slot_area, popup_area, status_area] = Layout::vertical([
        Constraint::Min(1),
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
    /// `openExternalEditor` interactive-mode.ts:3611): resolve `$VISUAL`/`$EDITOR` (falling back to
    /// `nano` on unix, `notepad` on Windows), write the buffer to a temp `*.pi.md`, tear the TUI down
    /// to release the terminal, run the editor (inheriting stdio), and — on a clean exit — reload the
    /// edited text (trailing newline stripped). The terminal is always restored, even on error. No
    /// `unsafe`, no new dependency (`std::process` + `std::fs`).
    pub fn open_external_editor(&mut self) -> Result<(), TuiError> {
        let editor_cmd = std::env::var("VISUAL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| if cfg!(windows) { "notepad".to_string() } else { "nano".to_string() });

        let current = self.state.editor.text();
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("cyrup-editor-{}.pi.md", std::process::id()));
        if std::fs::write(&tmp, &current).is_err() {
            self.state.transcript.push_status("external editor: could not write temp file");
            return self.draw_synchronized();
        }

        // Release the terminal (cooked mode, no inline viewport) so the editor owns the screen.
        self.restore()?;
        // `editor arg1 arg2 … tmp` — split on spaces to support `code --wait`-style commands.
        let mut parts = editor_cmd.split_whitespace();
        let status = parts.next().map(|bin| {
            std::process::Command::new(bin)
                .args(parts)
                .arg(&tmp)
                .status()
        });

        // Reload on a clean exit; keep the original buffer otherwise (Pi: non-zero exit = no change).
        if let Some(Ok(s)) = status
            && s.success()
            && let Ok(new_text) = std::fs::read_to_string(&tmp)
        {
            let trimmed = new_text.strip_suffix('\n').unwrap_or(&new_text);
            self.state.editor.set_text(trimmed);
        }
        let _ = std::fs::remove_file(&tmp);

        // Re-enter raw mode + bracketed paste + Kitty flags, then redraw the live region.
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

    /// The interactive event loop: `select!` over terminal input, the agent event stream, theme
    /// hot-reload, and cancellation (arch-10 §5). Renders with synchronized output. Submissions are
    /// routed to `session` (steer while streaming, else a fresh prompt; R-10-030).
    pub async fn run(
        &mut self,
        mut input: EventStream<InputEvent>,
        mut events: EventStream<AgentSessionEvent>,
        session: Arc<AgentSession>,
        mut theme_rx: Option<tokio::sync::watch::Receiver<Arc<ThemeData>>>,
        cancel: CancelToken,
    ) -> Result<(), TuiError> {
        self.draw_synchronized()?;
        // The spinner tick (spec/tui/01 §6.2 / §10): an 80 ms redraw used **only while** a status
        // indicator is active, so the Braille frame advances without a timer thread and an idle
        // session never busy-loops (the branch is `if`-gated on `indicator.is_active()`).
        let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
        spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // A live `!`/`!!` bash subprocess: its output receiver + a cancel token to kill it (`Esc`).
        // Kept as run-loop locals (not on `self`) so the `select!` borrow does not collide with the
        // input-arm `&mut self`.
        let mut bash_rx: Option<tokio::sync::mpsc::UnboundedReceiver<BashMsg>> = None;
        let mut bash_cancel: Option<CancelToken> = None;
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
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = spinner.tick(), if self.state.indicator.is_active() => {
                    self.draw_synchronized()?;
                }
                maybe_in = input.next() => {
                    let Some(ev) = maybe_in else { break };
                    match self.handle_input(&ev) {
                        AppAction::Quit => break,
                        AppAction::Suspend => self.suspend()?,
                        AppAction::OpenExternalEditor => self.open_external_editor()?,
                        AppAction::Interrupt => {
                            session.abort();
                            // Also kill a running bash child (the block was already marked cancelled
                            // in `apply_action`); the reader task's terminal `Done` clears `bash_rx`.
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
                            let ui = UserInput::text(text, InputSource::Tui);
                            if session.is_streaming().await {
                                let _ = session.steer(ui).await;
                            } else {
                                let _ = session.prompt_accepted(ui).await;
                            }
                        }
                        AppAction::Command(cmd) => self.execute_command(cmd, &session).await,
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
                maybe_ev = events.next() => {
                    let Some(ev) = maybe_ev else { continue };
                    self.ingest_event(&ev);
                    self.draw_synchronized()?;
                }
                ok = theme_changed => {
                    if ok && let Some(rx) = theme_rx.as_ref() {
                        let data = rx.borrow().clone();
                        self.set_theme(UiTheme::from_theme_data(&data, 0));
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

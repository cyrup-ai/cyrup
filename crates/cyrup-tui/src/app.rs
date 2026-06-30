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
use crate::keymap::{Action, Keymap, SelectKeymap};
use crate::selector::{ListSelector, Selector, SelectorKind, SelectorOutcome};
use crate::status::StatusLine;
use crate::theme::UiTheme;
use crate::transcript::{content_text, entry_line, TranscriptView};

/// The decision produced by feeding one input event to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    /// The user submitted a prompt (already optimistically shown in the transcript).
    Submit(String),
    /// The user requested an abort/interrupt of the in-flight run (Esc).
    Interrupt,
    /// The user requested to quit the session.
    Quit,
    /// State changed; the frame should be redrawn.
    Redraw,
    /// Nothing to do.
    None,
}

/// All retained UI state (the data half of the `state -> frame` split).
pub struct AppState {
    pub transcript: TranscriptView,
    pub editor: InputEditor,
    pub status: StatusLine,
    pub theme: UiTheme,
    pub keymap: Keymap,
    /// The selector binding table (`tui.select.*`, spec/tui/05 §10) consulted while a selector owns
    /// the input slot.
    pub select_keymap: SelectKeymap,
    /// The slash-command registry driving dispatch + autocomplete (rebuilt on `/reload`).
    pub commands: CommandRegistry,
    /// The active editor-swap selector, if any (spec/tui/05 §1.1): when `Some`, it replaces the
    /// editor in the bottom inline region and captures input until it confirms/cancels.
    pub selector: Option<ActiveSelector>,
    /// The current reasoning level (`off`…`xhigh`), preselected by the thinking selector and updated
    /// on confirm. The authoritative level lives on the agent/session at the L7 layer.
    pub thinking_level: String,
    /// Whether inline images are shown (vs. a text placeholder), toggled by the show-images selector.
    pub show_images: bool,
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
            theme,
            keymap: Keymap::default(),
            select_keymap: SelectKeymap::default(),
            commands: CommandRegistry::new(),
            selector: None,
            thinking_level: "medium".to_string(),
            show_images: true,
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
        let lines: Vec<Line<'static>> =
            committed.iter().map(|e| entry_line(e, &self.state.theme)).collect();
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
                self.state.editor.insert_str(s);
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
        match self.state.commands.dispatch(text) {
            Dispatch::Empty => AppAction::Redraw,
            Dispatch::Prompt(prompt) => {
                self.state.transcript.push_user(prompt.clone());
                AppAction::Submit(prompt)
            }
            Dispatch::Command { name, arg } => {
                let label = match arg {
                    Some(a) => format!("/{name} {a}"),
                    None => format!("/{name}"),
                };
                self.state.transcript.push_status(format!("command: {label}"));
                AppAction::Redraw
            }
            Dispatch::Bash { command, excluded } => {
                let marker = if excluded { "!!" } else { "!" };
                self.state.transcript.push_status(format!("bash ({marker}): {command}"));
                AppAction::Redraw
            }
        }
    }

    /// Resolve a global keymap action (R-10-024 Ctrl+C, R-10-030 abort).
    fn apply_action(&mut self, action: Action) -> AppAction {
        match action {
            Action::Quit => {
                self.state.should_quit = true;
                AppAction::Quit
            }
            Action::Interrupt => {
                self.state.transcript.discard_streaming();
                self.state.status.set_streaming(false);
                AppAction::Interrupt
            }
            // `app.clear` (Ctrl+C): clear the editor buffer; if it was already empty Pi treats a
            // second press as exit (double-Ctrl+C). Here a clear is always a redraw.
            Action::Clear => {
                self.state.editor.clear();
                AppAction::Redraw
            }
            // `app.suspend` / `app.tools.expand` / page scroll are surfaced as actions for the run
            // loop to handle (SIGTSTP, expand toggle, transcript paging); the chrome itself only
            // redraws. Full wiring of suspend + transcript scroll is tracked on the residual ledger.
            Action::Suspend
            | Action::ToolsExpand
            | Action::PageUp
            | Action::PageDown => AppAction::Redraw,
        }
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
        };
        self.state.selector = Some(ActiveSelector { kind, inner, saved_editor, restore_theme });
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
                self.confirm_selector(kind, &value);
                self.close_selector(false);
                AppAction::Redraw
            }
            SelectorOutcome::Cancel => {
                self.close_selector(true);
                AppAction::Redraw
            }
        }
    }

    /// Apply a confirmed selection. Theme is fully applied in-crate; thinking-level + show-images
    /// persistence reaches the agent/settings at the L7 binary layer, so here we record the choice in
    /// app state and surface a status line (the same seam `/think` cycling uses).
    fn confirm_selector(&mut self, kind: SelectorKind, value: &str) {
        match kind {
            SelectorKind::Theme => {
                self.set_theme(UiTheme::builtin(value));
                self.state.transcript.push_status(format!("theme → {value}"));
            }
            SelectorKind::Thinking => {
                self.state.thinking_level = value.to_string();
                self.state.status.set_thinking_level(value);
                self.state.transcript.push_status(format!("thinking → {value}"));
            }
            SelectorKind::ShowImages => {
                self.state.show_images = value == "yes";
                let label = if self.state.show_images { "inline" } else { "placeholder" };
                self.state.transcript.push_status(format!("images → {label}"));
            }
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
            AgentSessionEvent::AgentStart => self.state.status.set_streaming(true),
            AgentSessionEvent::AgentEnd { .. } => {
                self.state.status.set_streaming(false);
                self.state.transcript.commit_assistant(None);
            }
            AgentSessionEvent::TurnStart | AgentSessionEvent::TurnEnd { .. } => {}
            AgentSessionEvent::MessageStart { .. } | AgentSessionEvent::MessageEnd { .. } => {}
            AgentSessionEvent::MessageUpdate { assistant_message_event, .. } => {
                self.ingest_stream_event(assistant_message_event);
            }
            AgentSessionEvent::ToolExecutionStart { tool_name, .. } => {
                self.state.transcript.push_tool_start(tool_name.clone());
            }
            AgentSessionEvent::ToolExecutionUpdate { .. } => {}
            AgentSessionEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
                self.state.transcript.push_tool_end(tool_name.clone(), *is_error);
            }
            AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                self.state.status.set_queued(steering.len().saturating_add(follow_up.len()));
            }
            AgentSessionEvent::CompactionStart { .. } => {
                self.state.transcript.push_status("compacting context…");
            }
            AgentSessionEvent::CompactionEnd { .. } => {
                self.state.transcript.push_status("compaction complete");
            }
            AgentSessionEvent::AutoRetryStart { attempt, max_attempts, .. } => {
                self.state.transcript.push_status(format!("retrying ({attempt}/{max_attempts})…"));
            }
            AgentSessionEvent::AutoRetryEnd { success, .. } => {
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
    let footer_h = if area.height >= chrome_h.saturating_add(3) { 2 } else { 1 };
    let [msg_area, slot_area, popup_area, status_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(slot_h),
        Constraint::Length(popup_h),
        Constraint::Length(footer_h),
    ])
    .areas(area);
    state.transcript.render(frame, msg_area, &state.theme);
    if let Some(active) = state.selector.as_mut() {
        active.inner.render(frame, slot_area, &state.theme);
    } else {
        state.editor.render(frame, slot_area, &state.theme);
        if let Some(ac) = state.editor.autocomplete() {
            let lines = ac.list.lines(popup_area.width, &state.theme);
            frame.render_widget(Paragraph::new(lines).style(state.theme.base_style()), popup_area);
        }
    }
    state.status.render(frame, status_area, &state.theme);
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
        loop {
            let theme_changed = async {
                match theme_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => break,
                maybe_in = input.next() => {
                    let Some(ev) = maybe_in else { break };
                    match self.handle_input(&ev) {
                        AppAction::Quit => break,
                        AppAction::Interrupt => session.abort(),
                        AppAction::Submit(text) => {
                            let ui = UserInput::text(text, InputSource::Tui);
                            if session.is_streaming().await {
                                let _ = session.steer(ui).await;
                            } else {
                                let _ = session.prompt_accepted(ui).await;
                            }
                        }
                        AppAction::Redraw | AppAction::None => {}
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

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

use crate::component::{Component, InputEvent};
use crate::editor::{EditorOutcome, InputEditor};
use crate::error::TuiError;
use crate::keymap::{Action, Keymap};
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
            should_quit: false,
            scrollback: Vec::new(),
        }
    }
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
                if let Some(action) = self.state.keymap.action_for(key) {
                    return self.apply_action(action);
                }
                match self.state.editor.handle_key(key) {
                    EditorOutcome::Submit(text) => {
                        if text.trim().is_empty() {
                            return AppAction::Redraw;
                        }
                        self.state.transcript.push_user(text.clone());
                        AppAction::Submit(text)
                    }
                    EditorOutcome::Edited => AppAction::Redraw,
                    EditorOutcome::Ignored => AppAction::None,
                }
            }
            InputEvent::Paste(s) => {
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
        }
    }

    /// Fold an `AgentSessionEvent` into the UI state.
    ///
    /// Only the dependency-reachable parts are decoded (see [`crate::transcript`] dependency note):
    /// tool names + error flag, model changes, queue depth, compaction, and the **terminal**
    /// assistant message (recovered via `StreamEvent::terminal_message()`, which yields a
    /// `&cyrup_core::AssistantMessage`). Incremental delta text needs `cyrup-agent`/`cyrup-provider`
    /// in the dependency set and is fed meanwhile through
    /// [`TranscriptView::push_assistant_delta`](crate::transcript::TranscriptView::push_assistant_delta).
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
                // Terminal events carry the authoritative `AssistantMessage` (core type, reachable).
                if let Some(asst) = assistant_message_event.terminal_message() {
                    let text = content_text(&asst.content);
                    if !text.is_empty() {
                        self.state.transcript.commit_assistant(Some(text));
                    }
                    let tokens = asst.usage.total_tokens;
                    if tokens > 0 {
                        self.state.status.set_tokens(tokens);
                    }
                }
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
                self.state.transcript.push_status(format!("model → {label}"));
            }
            AgentSessionEvent::ThinkingLevelChanged { level } => {
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

}

/// Flatten a styled [`Line`] into its plain text (concatenated span content).
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Pure render: lay out conversation / editor / status and render each component (`state -> frame`).
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let editor_rows = (state.editor.line_count().min(u16::MAX as usize) as u16).saturating_add(2);
    let max_editor = area.height.saturating_sub(2).max(3);
    let editor_h = editor_rows.clamp(3, max_editor);
    let [msg_area, editor_area, status_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(editor_h),
        Constraint::Length(1),
    ])
    .areas(area);
    state.transcript.render(frame, msg_area, &state.theme);
    state.editor.render(frame, editor_area, &state.theme);
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

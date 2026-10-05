//! Pre-launch startup-selector runner (Pi `cli/startup-ui.ts` `createStartupTui` /
//! `showStartupSelector`, startup-ui.ts:77-163): mount ONE [`Selector`] over a short-lived
//! full-screen `CrosstermBackend` TUI, drive the crossterm event loop until the selector confirms or
//! cancels, then tear the terminal back down.
//!
//! This is the fixed, app-owned pre-launch surface Pi spins up BEFORE the agent runtime is built (the
//! `--resume` picker, the project-trust prompt, the missing-session-cwd selector). It is intentionally
//! *not* the in-app chrome ([`crate::App`]); it is a single modal selector with no transcript/editor.
//! The terminal and the key source are the only parts that need a real tty: the loop itself is
//! [`crate::startup_loop::StartupLoop`], generic over the ratatui backend and the key source, and is
//! what the tests step (SEAM-134).
//!
//! In-place `Apply` payloads (Pi's selectors that mutate a row in place — e.g. the resume picker's
//! delete/rename) are routed to the caller's `on_apply` and the slot stays open (the selector already
//! reflected the mutation in its own row list).

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::ExecutableCommand;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::Event;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::error::TuiError;
use crate::keymap::SelectKeymap;
use crate::selector::{Selector, SelectorOutcome};
use crate::session_selector::SessionSelector;
use crate::startup_loop::{LoadDriver, Retheme, StartupEvents, StartupLoop, StartupSessionLoads};
use crate::startup_theme::StartupTheme;
use crate::write_log::tui_stdout;

/// Restore the terminal on EVERY exit from [`run_startup_selector`] — the two setup errors, the
/// loop's `?`, and (new with `async`) a **future-drop**: the loop now suspends at each
/// `on_apply(..).await`, so the caller's future can be dropped mid-selector where the sync version
/// could not be. The old straight-line restore at the foot of the function ran on none of those.
///
/// Total and idempotent: every step is `let _ =` so a terminal that rejects one escape does not
/// stop the rest, and leaving an alternate screen that was never entered is harmless.
///
/// This does NOT cover a panic — `panic = "abort"` in the release profile means no unwind and no
/// `Drop`. [`crate::panic_hook::restore_terminal_best_effort`] is that path's only recourse.
struct StartupTerminalRestore;

impl Drop for StartupTerminalRestore {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = out.execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = out.execute(Show);
    }
}

/// Run a single `inner` selector to completion over a fresh full-screen terminal (Pi
/// `showStartupSelector`). Returns the terminal [`SelectorOutcome::Confirm`] / [`SelectorOutcome::Cancel`].
/// `on_apply` is invoked for each in-place [`SelectorOutcome::Apply`] payload (delete/rename) and the
/// loop continues. The terminal is always restored (raw mode off, alternate screen left, cursor
/// shown) by [`StartupTerminalRestore`] — on every exit, including a dropped future.
///
/// `async` because [`SelectorOutcome::Apply`] is now AWAITED: `on_apply` persists the mutation
/// before the loop repaints the row that shows it, so an in-place edit is durable before the frame
/// that reflects it is painted. The **input** wait is still a blocking one, so this parks its
/// executor thread between keys — unchanged from the sync version every caller already blocked
/// on, and NOT fixable in isolation: see the `.flux` task "unify the pre-launch input path with the
/// app reader" for why a second background reader on stdin is unsafe while
/// [`crate::app::crossterm_input_stream`] is coupled to `App::run`'s singleton statics.
///
/// On unix that read is the app's own byte reader ([`crate::input::reader::TtyReader`]), so the
/// selector frames and decodes keys exactly as the app will a moment later; elsewhere it is
/// crossterm's `event::poll` + `event::read()`.
pub async fn run_startup_selector(
    theme: &StartupTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    on_apply: impl AsyncFnMut(&str),
) -> Result<SelectorOutcome, TuiError> {
    let (outcome, _) = run_on_terminal(theme, keymap, inner, None, on_apply).await?;
    Ok(outcome)
}

/// [`run_startup_selector`] for the `--resume` picker, with its listing STREAMED (SEAM-134).
///
/// pi's `selectSession` mounts `SessionSelectorComponent` with two loaders and lets it fill itself
/// (`cli/session-picker.ts:15-55`, `session-selector.ts:869`, `:956-988` @v0.87.1): the picker is on
/// screen, reading `Loading …`, before the first session file has been read. `picker` must have been
/// built with [`SessionSelector::with_async_loaders`]; `loads` are the two loaders it asks for. Each
/// runs on the blocking pool, its batches are folded into the picker between keys, and whatever is
/// still loading is cancelled when the picker confirms or cancels.
///
/// Besides the outcome this returns every session's stored cwd any batch carried (session file path
/// to cwd): the caller has no finished scan to look the picked session's missing-cwd issue up in
/// (`main.ts:321-332`).
pub async fn run_startup_session_selector(
    theme: &StartupTheme,
    keymap: &SelectKeymap,
    picker: &mut SessionSelector,
    loads: StartupSessionLoads,
    on_apply: impl AsyncFnMut(&str),
) -> Result<(SelectorOutcome, HashMap<String, String>), TuiError> {
    run_on_terminal(
        theme,
        keymap,
        picker,
        Some(LoadDriver::new(loads)),
        on_apply,
    )
    .await
}

async fn run_on_terminal(
    theme: &StartupTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    loads: Option<LoadDriver>,
    on_apply: impl AsyncFnMut(&str),
) -> Result<(SelectorOutcome, HashMap<String, String>), TuiError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| TuiError::Backend(e.to_string()))?;
    // Armed the instant raw mode is on, so every exit below unwinds through `Drop`.
    let _restore = StartupTerminalRestore;
    stdout
        .execute(EnterAlternateScreen)
        .map_err(|e| TuiError::Backend(e.to_string()))?;
    // Frames go through the TUI's write channel — pi's startup UI renders through a
    // `ProcessTerminal` (`cli/startup-ui.ts:89` @v0.87.1), whose `write` `PI_TUI_WRITE_LOG` tees.
    let mut terminal = Terminal::new(CrosstermBackend::new(tui_stdout()))
        .map_err(|e| TuiError::Backend(e.to_string()))?;

    let mut input = Input::open()?;
    // pi `startStartupTui`: `ui.start()`, then at once `queryStartupTerminalColors` — asked, not
    // waited for. The reader just opened routes the terminal's reply to the query; the loop
    // re-applies the theme when it lands.
    let (colors_tx, colors_rx) = tokio::sync::mpsc::unbounded_channel();
    ask_for_colors(colors_tx);
    StartupLoop {
        terminal: &mut terminal,
        events: &mut input,
        theme: theme.theme(),
        retheme: Some(Retheme::new(
            theme.clone(),
            colors_rx,
            std::time::Instant::now(),
        )),
        keymap,
        inner,
        loads,
    }
    .run(on_apply)
    .await
}

/// Ask the terminal for its colours and return at once; the answer — whenever it comes — is sent on
/// `tx`. Asked of the terminal in raw mode with the reader running, which is what routes the reply.
#[cfg(unix)]
fn ask_for_colors(tx: tokio::sync::mpsc::UnboundedSender<crate::terminal_query::TerminalColors>) {
    crate::terminal_query::request_terminal_colors_async(
        tui_stdout(),
        std::sync::Arc::new(move |colors| {
            let _ = tx.send(colors);
        }),
    );
}

/// Windows has no byte reader to route a reply through, so there is nothing to ask: the wait for
/// colours times out and the system theme settles on what the environment says.
#[cfg(not(unix))]
fn ask_for_colors(_tx: tokio::sync::mpsc::UnboundedSender<crate::terminal_query::TerminalColors>) {}

/// The selector's key source.
#[cfg(unix)]
struct Input(crate::input::reader::TtyReader);

#[cfg(unix)]
impl Input {
    fn open() -> Result<Self, TuiError> {
        let timeout = crate::app::resolve_escape_timeout(|k| std::env::var(k).ok());
        crate::input::reader::TtyReader::open(timeout)
            .map(Self)
            .map_err(|e| TuiError::Backend(e.to_string()))
    }
}

#[cfg(unix)]
impl StartupEvents for Input {
    fn next(&mut self, wait: Duration) -> Result<Option<Event>, TuiError> {
        self.0
            .next_event_timeout(wait)
            .map_err(|e| TuiError::Backend(e.to_string()))
    }
}

/// The selector's key source.
#[cfg(not(unix))]
struct Input;

#[cfg(not(unix))]
impl Input {
    fn open() -> Result<Self, TuiError> {
        Ok(Self)
    }
}

#[cfg(not(unix))]
impl StartupEvents for Input {
    fn next(&mut self, wait: Duration) -> Result<Option<Event>, TuiError> {
        let backend = |e: io::Error| TuiError::Backend(e.to_string());
        if !ratatui::crossterm::event::poll(wait).map_err(backend)? {
            return Ok(None);
        }
        ratatui::crossterm::event::read().map(Some).map_err(backend)
    }
}

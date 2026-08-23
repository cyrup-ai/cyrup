//! Pre-launch startup-selector runner (Pi `cli/startup-ui.ts` `createStartupTui` /
//! `showStartupSelector`, startup-ui.ts:77-163): mount ONE [`Selector`] over a short-lived
//! full-screen `CrosstermBackend` TUI, drive the crossterm event loop until the selector confirms or
//! cancels, then tear the terminal back down.
//!
//! This is the fixed, app-owned pre-launch surface Pi spins up BEFORE the agent runtime is built (the
//! `--resume` picker, the project-trust prompt, the missing-session-cwd selector). It is intentionally
//! *not* the in-app chrome ([`crate::App`]); it is a single modal selector with no transcript/editor.
//! Like [`crate::App`] it needs a real terminal, so it is exercised from the bin, not unit-tested.
//!
//! In-place `Apply` payloads (Pi's selectors that mutate a row in place — e.g. the resume picker's
//! delete/rename) are routed to the caller's `on_apply` and the slot stays open (the selector already
//! reflected the mutation in its own row list).

use std::io::{self, Stdout};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::ExecutableCommand;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::error::TuiError;
use crate::keymap::SelectKeymap;
use crate::selector::{Selector, SelectorOutcome};
use crate::theme::UiTheme;

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
/// that reflects it is painted. The **input** read is still the blocking `event::read()`, so this
/// parks its executor thread between keys — unchanged from the sync version every caller already
/// blocked on, and NOT fixable in isolation: see the `.flux` task "unify the pre-launch input path
/// with the app reader" for why a second background reader on stdin is unsafe while
/// [`crate::app::crossterm_input_stream`] is coupled to `App::run`'s singleton statics.
pub async fn run_startup_selector(
    theme: &UiTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    on_apply: impl AsyncFnMut(&str),
) -> Result<SelectorOutcome, TuiError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| TuiError::Backend(e.to_string()))?;
    // Armed the instant raw mode is on, so every exit below unwinds through `Drop`.
    let _restore = StartupTerminalRestore;
    stdout
        .execute(EnterAlternateScreen)
        .map_err(|e| TuiError::Backend(e.to_string()))?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))
        .map_err(|e| TuiError::Backend(e.to_string()))?;

    run_loop(&mut terminal, theme, keymap, inner, on_apply).await
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: &UiTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    mut on_apply: impl AsyncFnMut(&str),
) -> Result<SelectorOutcome, TuiError> {
    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                // Pi passes `ui.terminal.rows` into `ConfigSelectorComponent`
                // (`cli/config-selector.ts:47`), which turns it into the body window
                // (`config-selector.ts:266`). Doing it here rather than at construction keeps the
                // window correct across a resize; it is a no-op for every other selector.
                inner.set_terminal_height(area.height);
                let height = inner.desired_height(area.width).min(area.height).max(1);
                let slot = Rect { x: area.x, y: area.y, width: area.width, height };
                inner.render(frame, slot, theme);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;

        match event::read().map_err(|e| TuiError::Backend(e.to_string()))? {
            // Ignore key-release events (Kitty protocol) so a single press is not double-counted.
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                match inner.handle(&key, keymap) {
                    SelectorOutcome::Confirm(value) => return Ok(SelectorOutcome::Confirm(value)),
                    SelectorOutcome::Cancel => return Ok(SelectorOutcome::Cancel),
                    SelectorOutcome::Apply(payload) => on_apply(&payload).await,
                    // Never produced by the startup selectors (`OpenExternalEditor` is only
                    // `ExtensionEditorSelector`'s; `OpenSubmenu` is only the `/settings` grid's) —
                    // treated as a no-op like `Redraw`'s siblings.
                    SelectorOutcome::Preview(_)
                    | SelectorOutcome::Redraw
                    | SelectorOutcome::Ignored
                    | SelectorOutcome::OpenExternalEditor
                    | SelectorOutcome::OpenSubmenu(_) => {}
                }
            }
            _ => {}
        }
    }
}

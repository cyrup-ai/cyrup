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

/// Run a single `inner` selector to completion over a fresh full-screen terminal (Pi
/// `showStartupSelector`). Returns the terminal [`SelectorOutcome::Confirm`] / [`SelectorOutcome::Cancel`].
/// `on_apply` is invoked for each in-place [`SelectorOutcome::Apply`] payload (delete/rename) and the
/// loop continues. The terminal is always restored (raw mode off, alternate screen left, cursor shown)
/// even on the error path.
pub fn run_startup_selector(
    theme: &UiTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    on_apply: impl FnMut(&str),
) -> Result<SelectorOutcome, TuiError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| TuiError::Backend(e.to_string()))?;
    if let Err(e) = stdout.execute(EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(TuiError::Backend(e.to_string()));
    }
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(t) => t,
        Err(e) => {
            let mut out = io::stdout();
            let _ = out.execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(TuiError::Backend(e.to_string()));
        }
    };

    let result = run_loop(&mut terminal, theme, keymap, inner, on_apply);

    // Restore — total and idempotent so any error path still leaves a usable terminal.
    let mut out = io::stdout();
    let _ = out.execute(LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = terminal.show_cursor();
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    theme: &UiTheme,
    keymap: &SelectKeymap,
    inner: &mut dyn Selector,
    mut on_apply: impl FnMut(&str),
) -> Result<SelectorOutcome, TuiError> {
    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
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
                    SelectorOutcome::Apply(payload) => on_apply(&payload),
                    // Never produced by the startup selectors (only `ExtensionEditorSelector`'s
                    // `handle` ever returns it) — treated as a no-op like `Redraw`'s siblings.
                    SelectorOutcome::Preview(_)
                    | SelectorOutcome::Redraw
                    | SelectorOutcome::Ignored
                    | SelectorOutcome::OpenExternalEditor => {}
                }
            }
            _ => {}
        }
    }
}

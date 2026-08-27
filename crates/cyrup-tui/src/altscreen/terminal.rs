//! The alternate screen itself — entering it, configuring it, and tearing it back down in pi's
//! exact order. cyrup's port of `TuiAltScreen`'s three terminal-lifecycle hooks
//! (`packages/tui/src/tui-alt-screen.ts` @v0.84.3: `beforeTerminalStart` `:257-295`,
//! `beforeTerminalStop` `:297-309`, `afterTerminalStop` `:311-333`). ADR-0005 §Decision B-3.
//!
//! # What "in pi's order" buys
//! The teardown is the half that matters. Upstream splits it in two because `TuiBase.stop`
//! (`tui.ts:752-762`) interleaves its own work between the hooks, and each half is written as ONE
//! bracketed write:
//!
//! 1. `beforeTerminalStop` (`:306`) — `?2026h`, the kitty-image deletes, the mouse disable,
//!    autowrap back ON, `?2026l`.
//! 2. `afterTerminalStop` (`:315` / `:322-327`) — `?2026h`, leave the alternate screen, then either
//!    nothing more (`preserveScreen`) or the last rendered document repainted onto the main screen,
//!    the cursor back, `?2026l`.
//!
//! The `?2026h`/`?2026l` bracket is not decoration: leaving the alternate screen makes the
//! terminal's saved main screen reappear, and every byte written after that point lands on the
//! user's shell. Without the bracket a slow terminal paints the restored screen, then the repaint,
//! then the cursor — three visible states for one logical transition. See
//! [`crate::panic_hook::restore_terminal_best_effort`] for the same reasoning applied to the
//! opposite hazard (an update opened and never closed).
//!
//! # What this module deliberately does NOT do
//! - **It never touches raw mode.** `startup_selector`'s guard (`startup_selector.rs:44-51`) calls
//!   `disable_raw_mode` because it owns a short-lived pre-session terminal that nothing outlives.
//!   Here [`crate::App`] owns raw mode across the whole session *including* the fullscreen
//!   excursion (ADR-0005 §B-14 switches renderers, it does not restart the process), so dropping
//!   raw mode on the way out would hand the inline renderer a cooked terminal.
//! - **It emits no mouse sequences** — ADR-0005 §B-4 (`mouse.rs`) owns those, and must emit its
//!   disable immediately BEFORE [`TerminalSetup::leave`] to sit where pi puts it (`:306`). Nothing
//!   is lost by it being a separate write: mouse-mode escapes produce no glyphs, so the
//!   synchronized bracket's tearing guarantee is unaffected.
//! - **It repaints nothing** — ADR-0005 §B-13 (`repaint.rs`) fills the row loop marked in
//!   [`TerminalSetup::leave`]'s `preserve_screen == false` branch (`:323-326`).
//! - **It renders nothing.** Constructing the [`Terminal`] is all this module does with the
//!   backend; every frame after that is the renderer's.
//!
//! # Where the escapes come from
//! Every sequence is a typed `crossterm` command rather than a hand-written string literal, which
//! is both the house idiom (`app/crossterm.rs`, `panic_hook.rs`, `startup_selector.rs:20-23`) and
//! what keeps this file free of the raw `\x1b[…]` byte-slicing the crate's
//! `deny(clippy::string_slice)` exists to discourage. The mapping to pi's constants
//! (`tui-alt-screen.ts:51-61`) is one-to-one and noted at each use site.

use std::io::{self, Write};

use ratatui::backend::Backend;
use ratatui::crossterm::cursor::{Hide, MoveTo, Show};
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{Print, ResetColor};
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, Clear, ClearType, DisableLineWrap, EnableLineWrap,
    EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::error::TuiError;

/// The alternate-screen terminal: a `Viewport::Fullscreen` [`Terminal`] over the caller's backend,
/// paired with the [`TerminalSetup`] guard that restores the screen when it goes away.
///
/// `Viewport::Fullscreen` is ratatui's default (`ratatui-core/src/terminal/viewport.rs:77-78`), so
/// this is a plain `Terminal::new` — deliberately none of the `TerminalOptions`/`RebuildBackend`
/// reanchor machinery `App` needs (`app/shell.rs:12-16`, `app/draw.rs:113-118`). That machinery
/// exists solely because `Viewport::Inline`'s height is fixed at construction and the live region
/// has to track its content; a fullscreen viewport resizes itself inside `Terminal::draw`.
///
/// # Field order is load-bearing
/// `terminal` is declared first and therefore dropped first, so [`TerminalSetup`]'s restore is the
/// last thing that runs — nothing can paint into the alternate screen after it has been left.
///
/// # The backend is not this type's to hand back
/// There is no `into_backend`: `ratatui::Terminal` has no consuming accessor, which is exactly why
/// ADR-0005 §B-14's mode switch builds the second terminal from `RebuildBackend::rebuild()` rather
/// than moving a backend between two `Terminal`s (see [`crate::ViewportRenderer`]'s scope note).
pub(super) struct AltTerminal<B: Backend> {
    terminal: Terminal<B>,
    setup: TerminalSetup,
}

impl<B: Backend> AltTerminal<B> {
    /// Enter the alternate screen and build the fullscreen terminal over `backend` — pi's
    /// `beforeTerminalStart` (`tui-alt-screen.ts:257-295`) followed by the `Terminal` construction
    /// upstream gets from its own `terminal.start()` (`tui.ts:698-705`).
    ///
    /// The guard is armed by [`TerminalSetup::enter`] BEFORE anything else can fail, so the `?` on
    /// the `Terminal::new` below unwinds through [`TerminalSetup`]'s `Drop` and leaves the user on
    /// their original screen with the cursor back — the `startup_selector.rs:73-78` idiom, where the
    /// guard is taken the instant the first terminal mode is touched rather than at the end of a
    /// successful setup.
    pub(super) fn enter(backend: B) -> Result<Self, TuiError> {
        let setup = TerminalSetup::enter()?;
        let terminal = Terminal::new(backend).map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(AltTerminal { terminal, setup })
    }

    /// Leave the alternate screen, restoring the terminal to its pre-[`Self::enter`] state.
    ///
    /// Idempotent and total; see [`TerminalSetup::leave`] for what `preserve_screen` selects and
    /// for why calling this explicitly is an optimisation over the `Drop` path rather than a
    /// requirement.
    pub(super) fn leave(&mut self, preserve_screen: bool) {
        self.setup.leave(preserve_screen);
    }

    /// The fullscreen [`Terminal`] every frame is drawn through.
    ///
    /// `&mut` only, and that is the whole accessor surface: ratatui's read paths (`size`, `backend`)
    /// are reachable through it by auto-reborrow, and handing out a `&Terminal` alongside would
    /// invite the borrow shape ADR-0005 §Part 4 R4 rules out — a painter that holds the terminal
    /// and the renderer state at once cannot exist, because `Frame` only lives inside the closure
    /// `Terminal::draw` hands out and that closure already holds the terminal (`app/draw.rs:89`).
    pub(super) fn terminal_mut(&mut self) -> &mut Terminal<B> {
        &mut self.terminal
    }
}

/// The armed restore for one alternate-screen excursion — pi's `altScreenActive` flag
/// (`tui-alt-screen.ts:179`, set at `:263`, cleared at `:313`) turned into an RAII guard.
///
/// Upstream reads that flag as an early-return in both stop hooks (`:304`, `:312`) so a second
/// `stop()` writes nothing; here the same flag is what makes [`Self::leave`] and `Drop` idempotent
/// with respect to each other. Every step of the restore is `let _ =`, exactly as
/// `StartupTerminalRestore` (`startup_selector.rs:44-51`) and
/// [`crate::panic_hook::restore_terminal_best_effort`] do: this can run while the process is already
/// failing, and a terminal that rejects one escape must not stop the rest — leaving the user inside
/// the alternate screen because an autowrap reset failed would be the worst outcome available.
///
/// **Not a substitute for the panic hook.** The release profile sets `panic = "abort"`
/// (workspace `Cargo.toml:296`), so no unwind and no `Drop` runs on a panic; this guard covers the `?` early
/// returns, an ordinary scope exit, and a dropped future — the same three
/// `StartupTerminalRestore` was written for (`startup_selector.rs:32-41`).
pub(super) struct TerminalSetup {
    /// pi's `altScreenActive` (`tui-alt-screen.ts:179`). `true` between a successful
    /// [`Self::enter`] and the first [`Self::leave`]; the restore runs at most once.
    active: bool,
}

impl TerminalSetup {
    /// Enter the alternate screen — pi's `beforeTerminalStart` write (`tui-alt-screen.ts:292-294`),
    /// which is `ENTER_ALT_SCREEN` + `DISABLE_AUTOWRAP` + (the mouse sequence, ADR-0005 §B-4) +
    /// `\x1b[2J\x1b[H` + `\x1b[?25l`, in that order and as one write.
    ///
    /// The guard is constructed FIRST so a failed write is still undone: the `?` below drops it and
    /// `Drop` emits the inverse. Leaving an alternate screen that was never entered is harmless, so
    /// over-restoring costs nothing and under-restoring costs the user their shell.
    ///
    /// The cursor is hidden with a raw `?25l` rather than `Terminal::hide_cursor`, matching pi and
    /// keeping ratatui's own `hidden_cursor` flag `false` — otherwise `Terminal`'s `Drop` would
    /// re-emit `Show` at an ordering this module does not control.
    fn enter() -> Result<Self, TuiError> {
        // Armed before the first byte, not after the last.
        let setup = TerminalSetup { active: true };
        let mut out = io::stdout();
        queue!(
            out,
            // `ENTER_ALT_SCREEN` (`:51`, `\x1b[?1049h`).
            EnterAlternateScreen,
            // `DISABLE_AUTOWRAP` (`:53`, `\x1b[?7l`): the renderer clips to the viewport itself, so
            // a line that overruns the last column must not wrap a row into the next one and shift
            // every row below it.
            DisableLineWrap,
            // `\x1b[2J\x1b[H` (`:293`). The alternate screen a terminal hands back is not
            // guaranteed blank, and ratatui's first frame diffs against an empty buffer — it writes
            // only non-blank cells, so anything already there would survive under the first frame.
            Clear(ClearType::All),
            MoveTo(0, 0),
            // `\x1b[?25l` (`:293`).
            Hide,
        )?;
        out.flush()?;
        Ok(setup)
    }

    /// Tear the alternate screen down — pi's `beforeTerminalStop` (`tui-alt-screen.ts:297-309`) and
    /// `afterTerminalStop` (`:311-333`) back to back, each in its own `?2026h`/`?2026l` bracket
    /// exactly as upstream writes them.
    ///
    /// `preserve_screen` is pi's `TuiStopOptions.preserveScreen` (`tui.ts:286-289`) — "leave
    /// renderer output in place for another TUI taking over the same terminal":
    ///
    /// - `true` (`:315`): exit the alternate screen and show the cursor, nothing else. The
    ///   terminal's own saved main screen reappears untouched.
    /// - `false` (`:322-327`): exit the alternate screen, then repaint the last rendered document
    ///   onto the main screen so the fullscreen session leaves its transcript in the user's
    ///   scrollback, then restore autowrap, emit the trailing newline and show the cursor. **The
    ///   row loop (`:323-326`) is ADR-0005 §B-13's** (`repaint.rs`), which owns the retained screen
    ///   lines; the framing around it is emitted here today, and with no rows to write it reduces
    ///   to upstream's own zero-line output — one `\r\n` and a reset.
    ///
    /// A second call writes nothing (pi's `if (!this.altScreenActive) return`, `:304`/`:312`), which
    /// is what lets the orderly path call this and `Drop` still be correct on the paths that do not.
    /// Returns nothing and swallows every write error for the reason given on the type.
    pub(super) fn leave(&mut self, preserve_screen: bool) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut out = io::stdout();

        // ---- pi `beforeTerminalStop` (`:305-307`) -------------------------------------------
        // `BEGIN_SYNCHRONIZED_OUTPUT` + kitty deletes (§B-12) + `DISABLE_MOUSE` (§B-4) +
        // `ENABLE_AUTOWRAP` + `END_SYNCHRONIZED_OUTPUT`. The two omitted middles belong to units
        // that emit them from their own modules immediately before this call; neither produces a
        // glyph, so their being a separate write does not weaken the bracket.
        let _ = queue!(out, BeginSynchronizedUpdate, EnableLineWrap, EndSynchronizedUpdate);

        // ---- pi `afterTerminalStop` (`:314-329`) --------------------------------------------
        let _ = queue!(out, BeginSynchronizedUpdate, LeaveAlternateScreen);
        if !preserve_screen {
            // `DISABLE_AUTOWRAP` again for the repaint (`:322`): the document rows are written to
            // the main screen at their rendered width, and a wrap here would double-space the
            // history the user is about to scroll through.
            let _ = queue!(out, DisableLineWrap);
            // ADR-0005 §B-13 (`repaint.rs`) writes the retained screen here, one row per iteration
            // of `:323-326`: `"\r\n"` between rows, then `"\r\x1b[2K"` and the row's text.
            // `\x1b[0m` + `ENABLE_AUTOWRAP` + `"\r\n"` (`:327`) close the repaint whether or not any
            // row was written — with none, this is upstream's output for an empty document.
            let _ = queue!(out, ResetColor, EnableLineWrap, Print("\r\n"));
        }
        // `\x1b[?25h` + `END_SYNCHRONIZED_OUTPUT` (`:315`, `:327`) close both branches.
        let _ = queue!(out, Show, EndSynchronizedUpdate);
        let _ = out.flush();
    }
}

impl Drop for TerminalSetup {
    /// The un-taken exit: a `?` early return inside setup, an ordinary scope exit, or a dropped
    /// future. A [`TerminalSetup::leave`] that already ran cleared `active`, so this is a no-op on
    /// the orderly path.
    ///
    /// Restores with `preserve_screen == true` — the branch that exits the alternate screen and
    /// stops. That is the only correct choice here and not a shortcut: the `false` branch exists to
    /// repaint the last rendered document (ADR-0005 §B-13), and on an unwind there is no rendered
    /// document to repaint. Bringing the user's own screen back is what upstream's `preserveScreen`
    /// branch does (`tui-alt-screen.ts:315`).
    fn drop(&mut self) {
        self.leave(true);
    }
}

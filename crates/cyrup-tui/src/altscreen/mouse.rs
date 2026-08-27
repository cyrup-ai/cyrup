//! Mouse and focus reporting for the alternate screen — cyrup's port of the three mouse constants
//! and the multiplexer branch that selects between them
//! (`packages/tui/src/tui-alt-screen.ts` @v0.84.3: `:55-57`, selected at `:281-292`, written at
//! `:293`, undone at `:306`). ADR-0005 §Decision B-4.
//!
//! # Why the literals, and not crossterm's mouse-capture command
//! crossterm's `Enable`/`DisableMouseCapture` pair is the wrong sequence in three separate ways,
//! and this is the one place in the fullscreen feature where the obvious call is a defect rather
//! than a shortcut. The enable half (crossterm 0.29, `src/event.rs`) emits
//! `?1000h ?1002h ?1003h ?1015h ?1006h` unconditionally:
//!
//! 1. It always turns on **any-motion tracking (`?1003h`)**. pi deliberately does not under a
//!    multiplexer, because forwarding every pointer movement through tmux/zellij/screen makes them
//!    lag — and button-motion tracking (`?1002h`) still preserves clicks, wheel events, selections
//!    and scrollbar drags, which is the whole of what this renderer reads (`:282-283`).
//! 2. It adds rxvt-style reporting (`?1015h`), which pi never asks for; SGR (`?1006h`) is the only
//!    encoding the alternate screen parses.
//! 3. It never enables **focus reporting (`?1004h`)**, and the alternate screen's input handler
//!    depends on `FOCUS_OUT` to cancel an in-progress selection and a live scrollbar drag
//!    (`:543-561`). Without it, a drag that leaves the window never ends.
//!
//! Emitting the literals is not a fork of crossterm and not a workaround: crossterm `Command`s are
//! plain ANSI writers, so this is the same `queue!` of escapes with a different payload — one that
//! [`crate::altscreen::terminal`] already uses for every sequence it owns.
//!
//! # Where these sit in the lifecycle
//! Upstream folds the enable into the middle of its single `beforeTerminalStart` write, between
//! `DISABLE_AUTOWRAP` and `\x1b[2J\x1b[H\x1b[?25l` (`:293`), and the disable into the middle of its
//! `beforeTerminalStop` bracket, between the kitty deletes and `ENABLE_AUTOWRAP` (`:306`). cyrup
//! splits those writes across two modules, so [`MouseSetup::enable`] runs immediately AFTER
//! `AltTerminal::enter` and [`MouseSetup::disable`] immediately BEFORE `AltTerminal::leave`. The
//! position delta is unobservable for the reason `terminal.rs` gives for the same split:
//! mouse-mode and focus-mode escapes produce no glyphs, so nothing about the synchronized-update
//! bracket's tearing guarantee depends on them being in the same write as the painting ones.
//!
//! # What this module does NOT do
//! It never interprets a report. Decoding is crossterm's (the reader thread already parses SGR
//! mouse reports into `Event::Mouse` — see [`map_reader_event`]), and dispatching a decoded event to
//! the wheel, drag and selection handlers is ADR-0005 §B-3's single `match`. This module owns
//! exactly one thing: whether the terminal is sending reports at all, and the process-global answer
//! to that question which the input reader consults.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::crossterm::event::MouseEvent;
use ratatui::crossterm::queue;
use ratatui::crossterm::style::Print;

use crate::component::InputEvent;
use crate::error::TuiError;

/// pi's `ENABLE_BUTTON_MOTION_MOUSE` (`tui-alt-screen.ts:55`) — the multiplexer sequence.
///
/// `?1000h` button press/release, `?1002h` motion **while a button is held**, `?1004h` focus
/// in/out, `?1006h` SGR encoding. No `?1003h`: see the module doc.
const ENABLE_BUTTON_MOTION_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1004h\x1b[?1006h";

/// pi's `ENABLE_ALL_MOTION_MOUSE` (`tui-alt-screen.ts:56`) — [`ENABLE_BUTTON_MOTION_MOUSE`] plus
/// `?1003h`, any-motion tracking, which is what lets the scrollbar highlight on hover with no
/// button held (ADR-0005 §B-7).
const ENABLE_ALL_MOTION_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h";

/// pi's `DISABLE_MOUSE` (`tui-alt-screen.ts:57`) — every mode either enable sequence can set, reset
/// in the reverse of the order it was set in.
///
/// Emitted whole in both branches, exactly as upstream does (`:306` has no second variant):
/// resetting `?1003l` after the multiplexer branch never set `?1003h` is a no-op at the terminal,
/// and one unconditional inverse cannot drift out of step with two enable sequences.
const DISABLE_MOUSE: &str = "\x1b[?1006l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// Process-global mirror of pi's `mouseEnabled` (`tui-alt-screen.ts:198`, set at `:221`) narrowed
/// to what it means downstream: **the terminal is currently sending us reports because we asked it
/// to**.
///
/// A `static` rather than a field threaded to the reader, for the reason
/// [`crate::terminal_progress`]'s `PROGRESS_ARMED` (`terminal_progress.rs:84`) is one: the consumer
/// is [`crate::app::input_reader::map_event_on`], which runs on the reader `std::thread` and is
/// reached through an `EventStream` whose type carries no handle to thread one down
/// (`app/input_reader.rs:28-50`). There is exactly one interactive renderer per process, so a
/// singleton is what is being expressed.
///
/// Written only by [`MouseSetup`], so it can never disagree with what was actually sent to the
/// terminal. `Relaxed` is sufficient: the reader only asks "are reports expected", and orders no
/// other memory against the answer.
static REPORTING: AtomicBool = AtomicBool::new(false);

/// Whether mouse reporting is on — i.e. whether the alternate-screen renderer asked for it and has
/// not yet given it back.
///
/// `false` for the whole of every regular-mode session, which is what makes [`map_reader_event`]
/// byte-identical to the unconditional discard it replaced.
pub(crate) fn reporting_enabled() -> bool {
    REPORTING.load(Ordering::Relaxed)
}

/// Whether this process is running under a terminal multiplexer — pi's four-way test at
/// `tui-alt-screen.ts:284-289`: `TMUX`, `ZELLIJ` or `STY` set, or a lowercased `TERM` beginning
/// `tmux` or `screen`.
///
/// `[CYRUP-DELTA]`: the `TMUX` half is [`crate::tmux::in_tmux`] (`tmux.rs:51`), which treats an
/// EMPTY `TMUX` as "not in tmux" — pi's own `interactive-mode.ts:941` semantics — where this call
/// site instead tests `!== undefined` and would count it. Nothing is lost: tmux always exports a
/// non-empty `TMUX` alongside a `screen`/`tmux` `TERM`, so the prefix test below is the backstop,
/// and reusing the existing probe keeps one definition of "in tmux" in the crate. `ZELLIJ` and
/// `STY` keep upstream's set-or-not test, since neither has a second reader here to agree with.
pub(super) fn under_multiplexer() -> bool {
    if crate::tmux::in_tmux()
        || std::env::var_os("ZELLIJ").is_some()
        || std::env::var_os("STY").is_some()
    {
        return true;
    }
    // pi lowercases before the prefix test (`:281`), so `TERM=SCREEN-256color` counts.
    let term = match std::env::var("TERM") {
        Ok(value) => value.to_lowercase(),
        Err(_) => String::new(),
    };
    term.starts_with("tmux") || term.starts_with("screen")
}

/// The enable sequence this environment gets — pi's `mouseSequence` ternary
/// (`tui-alt-screen.ts:284-291`).
pub(super) fn enable_sequence() -> &'static str {
    if under_multiplexer() {
        ENABLE_BUTTON_MOTION_MOUSE
    } else {
        ENABLE_ALL_MOTION_MOUSE
    }
}

/// The armed mouse/focus reporting for one alternate-screen excursion, as an RAII guard — the same
/// shape, and for the same reasons, as `terminal.rs`'s `TerminalSetup`.
///
/// Every step of the restore is `let _ =`: this can run while the process is already failing, and a
/// terminal that rejects one escape must not stop the rest — leaving a terminal in `?1003h` after
/// the process exits means the user's shell fills with mouse reports on the next pointer movement.
///
/// **Not a substitute for the panic hook.** The release profile sets `panic = "abort"`, so no
/// `Drop` runs on a panic; this covers the `?` early returns, an ordinary scope exit and a dropped
/// future, exactly as `startup_selector.rs:32-41` describes for its own guard.
pub(super) struct MouseSetup {
    /// pi's `altScreenActive` guard applied to this half (`:304`): `true` between a successful
    /// [`Self::enable`] and the first [`Self::disable`], so the reset runs at most once.
    active: bool,
}

impl MouseSetup {
    /// Turn mouse and focus reporting on — the `${this.mouseEnabled ? mouseSequence : ""}` slice of
    /// pi's `beforeTerminalStart` write (`tui-alt-screen.ts:293`).
    ///
    /// Call immediately after `AltTerminal::enter` and before the first frame; see the module doc
    /// for why splitting it out of that write changes nothing observable.
    ///
    /// The guard is constructed, and [`REPORTING`] armed, BEFORE the first byte — so a failed write
    /// unwinds through `Drop`, which emits the inverse and disarms. Over-resetting a mode the
    /// terminal never entered costs nothing; under-resetting costs the user a shell that types
    /// escape sequences at them.
    pub(super) fn enable() -> Result<Self, TuiError> {
        let setup = MouseSetup { active: true };
        REPORTING.store(true, Ordering::Relaxed);
        let mut out = io::stdout();
        queue!(out, Print(enable_sequence()))?;
        out.flush()?;
        Ok(setup)
    }

    /// Turn reporting back off — the `${this.mouseEnabled ? DISABLE_MOUSE : ""}` slice of pi's
    /// `beforeTerminalStop` write (`tui-alt-screen.ts:306`).
    ///
    /// Call immediately BEFORE `AltTerminal::leave`, which is where upstream puts it: the reset
    /// belongs inside the teardown bracket, ahead of `LeaveAlternateScreen`, so no report can
    /// arrive addressed to a screen that is already gone.
    ///
    /// A second call writes nothing (pi's `if (!this.altScreenActive) return`, `:304`), which is
    /// what lets the orderly path call this and `Drop` still be correct on the paths that do not.
    /// Swallows every write error for the reason given on the type.
    pub(super) fn disable(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        // Disarmed before the write, not after: the reader must stop accepting reports even if the
        // reset escape never reaches the terminal.
        REPORTING.store(false, Ordering::Relaxed);
        let mut out = io::stdout();
        let _ = queue!(out, Print(DISABLE_MOUSE));
        let _ = out.flush();
    }
}

impl Drop for MouseSetup {
    /// The un-taken exit: a `?` early return during setup, an ordinary scope exit, or a dropped
    /// future. A [`MouseSetup::disable`] that already ran cleared `active`, so this is a no-op on
    /// the orderly path.
    fn drop(&mut self) {
        self.disable();
    }
}

/// The input reader's mouse arm — what `Event::Mouse` becomes now that something in this process
/// asks for the reports (`app/input_reader.rs`, replacing an unconditional `=> None`).
///
/// Gated on [`reporting_enabled`], so the inline renderer is unaffected in the strongest available
/// sense: in a session that never enters the alternate screen the flag is never set, and this
/// returns `None` for every report exactly as the discard it replaced did. Reports cannot even
/// arrive there — cyrup enables no mouse mode outside this module — so the gate is belt and braces
/// against a terminal left in a reporting mode by a previous program.
///
/// # Routing
/// The armed branch carries the report as [`InputEvent::Mouse`], which `App::handle_input` hands to
/// [`crate::AltScreen::handle_mouse`] — §B-3's dispatcher over `scrollbar_drag`, `selection` and
/// `wheel`, in upstream's `:526-604`-before-`:605-963` order.
pub(crate) fn map_reader_event(ev: MouseEvent) -> Option<InputEvent> {
    if !reporting_enabled() {
        return None;
    }
    Some(InputEvent::Mouse(ev))
}

//! Colour-scheme change notifications: DEC private mode `2031` and the report it elicits.
//!
//! Pi enables the mode while the terminal's appearance should be followed — an automatic
//! `light/dark` theme pair, or the `system` theme — and a terminal that supports it then sends
//! `CSI ? 997 ; 1 n` (dark) or `CSI ? 997 ; 2 n` (light) whenever its appearance changes
//! (`tui.ts:925-927`, `:950-957`, `:973-975` @v1.0.0). The controller answers a report by recording
//! the scheme and asking the terminal for its colours again (`theme-controller.ts:240-248`).
//!
//! This module holds the two halves the app needs:
//!
//! - [`Notifications`], the lifecycle of the mode, as a value. Pi keeps three facts: whether the
//!   notifications are wanted (`terminalColorSchemeNotificationsEnabled`), whether the terminal is
//!   running (`!stopped`), and, implicitly, what it last wrote. The mode is on the wire exactly
//!   while it is wanted and the terminal is running, so every transition answers with the one
//!   escape it owes, or nothing. `start()` re-asserts it, `stop()` and `setTerminalColorScheme
//!   Notifications(false)` take it down, and a toggle while stopped writes nothing and is picked up
//!   by the next `start()`.
//! - the report's route from the byte reader to the run loop: [`offer_report`] recognises a framed
//!   `CSI ? 997 ; N n` and [`deliver`] hands the scheme to whoever [`set_listener`] installed.
//!
//! # Where the mode is turned on and off
//!
//! | event | pi | here |
//! |---|---|---|
//! | the terminal starts | `TUI.start()` | [`terminal_started`], from `App::into_stdout` |
//! | the terminal stops (quit, panic, crash) | `TUI.stop()` | [`terminal_stopped`], from [`crate::panic_hook::restore_terminal_best_effort`] |
//! | suspend / resume, `$EDITOR` | `ui.stop()` / `ui.start()` | the same two calls around them |
//! | the alternate screen is entered or left | the old TUI stops, the new one starts, `rebindTui()` | `TerminalSetup::enter` / `leave` |
//! | the theme setting changes | `setAutoSync(enabled)` | [`set_notifications`], from `App::sync_color_scheme_notifications` |
//! | the Windows console reader opens or closes, or VT input goes on or off with the terminal's start and stop | `enableWindowsVTInput` in `ProcessTerminal.start()` | [`set_receivable`], from `crate::input::console_win`; inside [`terminal_started`] |
//!
//! # Windows (TUI-163)
//!
//! The report can be received only by a reader that sees the terminal's bytes. On unix that is
//! always the case. On Windows it is the case only while the console reader runs with
//! `ENABLE_VIRTUAL_TERMINAL_INPUT` (`crate::input::console_win`): crossterm's own console reader takes
//! each byte of a VT stream as a typed key, so a report pushed at it would land in the prompt. The
//! mode is therefore requested on Windows exactly while that reader is active — [`set_receivable`]
//! re-settles it when the reader starts, and when it stops or yields the console to `$EDITOR`. A
//! console that refuses virtual-terminal input keeps crossterm's reader, and the mode is never
//! requested there, as before.
//!
//! [CYRUP-DELTA] pi has no such gate: it requests the mode whenever the appearance is followed, and
//! when its `enableWindowsVTInput` fails (`terminal.ts:383-388` @v1.0.0 swallows the error) the
//! reports it asked for reach the prompt as typing. cyrup asks only while a reader can take them out
//! of the stream — the same reports on every console that accepts virtual-terminal input, and none
//! typed into the prompt on one that does not.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::theme::TerminalTheme;

/// `CSI ? 2031 h` — ask the terminal to report appearance changes (`tui.ts:926`).
pub(crate) const ENABLE: &str = "\x1b[?2031h";
/// `CSI ? 2031 l` — stop the reports (`tui.ts:956`, `:974`).
pub(crate) const DISABLE: &str = "\x1b[?2031l";

/// Whether the report can be received before any reader says otherwise: always on unix, and on
/// Windows only once the VT console reader is running (see the module docs).
const RECEIVABLE_AT_START: bool = cfg!(unix);

/// The lifecycle of mode `2031`: what is wanted, whether the terminal is running, and — derived —
/// whether the mode is on the wire.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notifications {
    /// Pi `terminalColorSchemeNotificationsEnabled` (`tui.ts:518`): an appearance-following theme is
    /// active.
    wanted: bool,
    /// Pi `!this.stopped` (`tui.ts:917`, `:970`).
    running: bool,
    /// TUI-163 — the input reader can take a report out of the stream (see the module docs). No pi
    /// counterpart: pi's input is always a byte stream.
    receivable: bool,
    /// What the terminal was last told.
    on_wire: bool,
}

impl Notifications {
    /// A terminal that has not started and wants nothing.
    pub(crate) const fn new() -> Self {
        Self {
            wanted: false,
            running: false,
            receivable: RECEIVABLE_AT_START,
            on_wire: false,
        }
    }

    /// Whether the notifications are wanted (pi's flag), irrespective of the terminal's state.
    #[cfg(test)]
    pub(crate) const fn wanted(&self) -> bool {
        self.wanted
    }

    /// Whether the mode is currently on.
    #[cfg(test)]
    pub(crate) const fn on_wire(&self) -> bool {
        self.on_wire
    }

    /// Bring the wire in line with `wanted && running`, answering the escape that takes.
    fn settle(&mut self) -> Option<&'static str> {
        let desired = self.wanted && self.running && self.receivable;
        if desired == self.on_wire {
            return None;
        }
        self.on_wire = desired;
        Some(if desired { ENABLE } else { DISABLE })
    }

    /// Pi `setTerminalColorSchemeNotifications(enabled)` (`tui.ts:950-957`): record the wish; the
    /// escape is written only while the terminal runs.
    pub(crate) fn set_wanted(&mut self, enabled: bool) -> Option<&'static str> {
        self.wanted = enabled;
        self.settle()
    }

    /// TUI-163 — the input reader can (or can no longer) take a report out of the stream.
    pub(crate) fn set_receivable(&mut self, receivable: bool) -> Option<&'static str> {
        self.receivable = receivable;
        self.settle()
    }

    /// Pi `TUI.start()` (`tui.ts:917-929`): the terminal is running again.
    pub(crate) fn start(&mut self) -> Option<&'static str> {
        self.running = true;
        self.settle()
    }

    /// Pi `TUI.stop()` (`tui.ts:970-980`): the terminal is released or gone. The wish survives, so
    /// the next [`Self::start`] turns the mode back on.
    pub(crate) fn stop(&mut self) -> Option<&'static str> {
        self.running = false;
        self.settle()
    }
}

/// The process-wide state: one terminal, one mode.
static STATE: Mutex<Notifications> = Mutex::new(Notifications::new());

fn with_state<T>(f: impl FnOnce(&mut Notifications) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

fn write_to(out: &mut impl Write, escape: Option<&'static str>) {
    if let Some(escape) = escape {
        let _ = out.write_all(escape.as_bytes());
        let _ = out.flush();
    }
}

/// Pi `setTerminalColorSchemeNotifications(enabled)` (`tui.ts:950-957`), written to the TUI's
/// channel (pi's `this.terminal.write`).
pub(crate) fn set_notifications(enabled: bool) {
    let escape = with_state(|s| s.set_wanted(enabled));
    write_to(&mut crate::write_log::tui_stdout(), escape);
}

/// TUI-163 — the input reader can (or can no longer) receive the report; the mode follows at once,
/// written to the TUI's channel.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn set_receivable(receivable: bool) {
    let escape = with_state(|s| s.set_receivable(receivable));
    write_to(&mut crate::write_log::tui_stdout(), escape);
}

/// Pi `TUI.start()` for the terminal this process owns.
pub(crate) fn terminal_started(out: &mut impl Write) {
    // TUI-163 — on Windows the start is also when the console reader's VT input is turned back on,
    // and with it whether the report can be received.
    #[cfg(windows)]
    let receivable = Some(crate::input::console_win::claim());
    #[cfg(not(windows))]
    let receivable = None;
    let escape = with_state(|s| {
        let changed = receivable.and_then(|r| s.set_receivable(r));
        changed.or(s.start())
    });
    write_to(out, escape);
}

/// Pi `TUI.stop()` for the terminal this process owns.
///
/// Callable from the panic hook: if the state lock is held by the thread that panicked, the mode is
/// switched off unconditionally — turning an already-off mode off again is harmless, and a terminal
/// left reporting into a dead process is not.
pub(crate) fn terminal_stopped(out: &mut impl Write) {
    let escape = match STATE.try_lock() {
        Ok(mut guard) => guard.stop(),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().stop(),
        Err(std::sync::TryLockError::WouldBlock) => Some(DISABLE),
    };
    write_to(out, escape);
    // TUI-163 — and the console's VT input goes back off, so whoever reads it next (the parent
    // shell, `$EDITOR`) gets keys, not escape sequences. The reader stays open, so the report stays
    // receivable for the next start.
    #[cfg(windows)]
    crate::input::console_win::release();
}

/// Whether the notifications are currently wanted (test/inspection).
#[cfg(test)]
pub(crate) fn wanted_for_test() -> bool {
    with_state(|s| s.wanted())
}

/// A running terminal that wants the notifications and has told the terminal so — the state a
/// session on the system theme is in.
#[cfg(test)]
pub(crate) fn arm_for_test() {
    with_state(|s| {
        s.start();
        s.set_wanted(true);
    });
}

/// Whether the mode is on the wire right now (test/inspection; its readers are unix tests).
#[cfg(all(test, unix))]
pub(crate) fn on_wire_for_test() -> bool {
    with_state(|s| s.on_wire())
}

/// Forget everything — a test that drives the process-wide state starts from here.
#[cfg(test)]
pub(crate) fn reset_for_test() {
    with_state(|s| *s = Notifications::new());
    set_listener(None);
}

// ============================================================================
// The report
// ============================================================================

/// Who hears about a report. Installed by the run loop for its lifetime.
pub(crate) type Listener = Arc<dyn Fn(TerminalTheme) + Send + Sync>;

static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// Install (or, with `None`, remove) the run loop's listener — pi's `onTerminalColorSchemeChange`
/// subscription (`tui.ts:943-948`).
pub(crate) fn set_listener(listener: Option<Listener>) {
    *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = listener;
}

/// Pi `consumeTerminalColorSchemeReport` (`tui.ts:1168-1178`), without the delivery: the scheme a
/// framed escape sequence reports, or `None` when it is not a colour-scheme report. The reader
/// swallows a report whether or not anyone listens, as pi's input handler does.
pub(crate) fn offer_report(sequence: &[u8]) -> Option<TerminalTheme> {
    if !sequence.starts_with(b"\x1b[?997;") {
        return None;
    }
    crate::terminal_query::parse_color_scheme_report(std::str::from_utf8(sequence).ok()?)
}

/// Hand `scheme` to the listener, if one is installed.
pub(crate) fn deliver(scheme: TerminalTheme) {
    let listener = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(listener) = listener {
        listener(scheme);
    }
}

/// The lock behind [`lock_for_test`] and [`lock_for_test_async`]: one mutex, so a synchronous test
/// and an async one exclude each other.
#[cfg(test)]
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serialises the tests that drive the process-wide state above (`cargo test` runs them on
/// threads of one process; nextest isolates each in its own).
#[cfg(test)]
pub(crate) fn lock_for_test() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK.blocking_lock()
}

/// [`lock_for_test`] for a test that is itself async and holds the lock across an await.
#[cfg(test)]
pub(crate) async fn lock_for_test_async() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().await
}

#[cfg(test)]
#[path = "color_scheme_tests.rs"]
mod tests;

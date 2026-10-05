//! Terminal restoration on an abnormal exit — ports pi's `uncaughtCrash` handler
//! (`interactive-mode.ts:3691-3708`, installed at `:3750-3755`).
//!
//! # The failure this prevents
//!
//! [`crate::App::into_stdout`] turns on raw mode, bracketed paste and the Kitty keyboard
//! disambiguation flags. [`crate::App::restore`] turns all three back off and shows the cursor, and
//! its doc calls itself "total and idempotent so a `Drop` guard / error path always leaves a usable
//! terminal". That is true for every path the app *returns* through — and reaches none of the paths
//! it does not.
//!
//! A panic is one of those. The user is left with raw mode on, bracketed paste on, Kitty flags
//! pushed and the cursor hidden: keystrokes stop echoing, Enter stops starting a new line, and
//! pasted text arrives wrapped in `200~`/`201~` markers. The shell is unusable until they blind-type
//! `stty sane; reset` — which is precisely the recovery pi's own handler doc names.
//!
//! cyrup is strictly WORSE off than pi here, and the reason is worth stating: the release profile
//! sets `panic = "abort"`, so there is no unwind, no `Drop`, and therefore no guard that could
//! possibly run. A panic hook is the ONLY mechanism that still executes — `std::panic::set_hook`
//! runs before the abort — which is why this module exists rather than a `Drop` impl.
//!
//! The workspace's no-panic clippy policy (`unwrap_used`/`expect_used`/`panic`/`indexing_slicing`
//! all denied) makes a first-party panic unlikely, and that is exactly why this is easy to leave
//! undone. It does not cover dependencies: an arithmetic overflow in a decoder, a slice assert in a
//! rendering crate, or a re-panic on a poisoned mutex all land here, and none of them are reachable
//! by auditing this workspace's own code.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::crossterm::ExecutableCommand;
use ratatui::crossterm::event::{DisableBracketedPaste, PopKeyboardEnhancementFlags};
use ratatui::crossterm::terminal::{EndSynchronizedUpdate, disable_raw_mode};

/// Undo everything [`crate::App::into_stdout`] turned on, best-effort and in reverse order.
///
/// Every step is `let _ =`: this runs while the process is already failing, so a terminal that
/// rejects one escape (a legacy terminal that never accepted the Kitty push, say) must not stop the
/// remaining steps — leaving raw mode on because a flag pop failed would be the worst outcome.
///
/// Ordering mirrors [`crate::App::restore`] exactly, and the two are kept in one place for that
/// reason: a future `into_stdout` that enables a fourth mode has to be undone in BOTH, and a
/// divergence would only ever be discovered by a user whose terminal was already broken.
///
/// Deliberately does NOT touch the alternate screen: the production app runs an inline viewport and
/// never enters it (only `startup_selector` does, and it owns its own exit path).
///
/// It DOES clear the OSC 9;4 progress indicator, which is not a terminal *mode* like the other
/// three: it is state the terminal emulator keeps on the app's behalf, and it survives the process
/// and every `stty sane`/`reset` a user would reach for. See that statement below.
///
/// It DOES also close any open synchronized update (DECSET 2026) — ahead of the OSC 9;4 clear and
/// of everything else, because an exit taken mid-frame leaves the terminal buffering every write
/// that follows, this function's own included. See the first statement below.
pub fn restore_terminal_best_effort() {
    restore_into(&mut crate::dead_terminal::terminal_stdout());
}

/// [`restore_terminal_best_effort`] over any sink — the escapes it writes, in the order it writes
/// them.
fn restore_into(out: &mut impl Write) {
    // Close the synchronized update FIRST, before this function writes anything else.
    // [`crate::App::draw_synchronized`] (`app/crossterm.rs:87-100`) brackets every frame in
    // `BeginSynchronizedUpdate` … `EndSynchronizedUpdate`, and both a hard exit through the TUI-092
    // input-reader escape hatch and a panic can land INSIDE that window — are LIKELY to, in fact,
    // since the wedge the escape hatch exists for is a run loop stuck mid-frame. With the closing
    // marker never emitted DECSET 2026 stays set and the terminal simply stops painting: the OSC
    // 9;4 clear below, the mode resets after it, the panic message the chained hook prints and the
    // user's next shell prompt are all held back until the terminal's own BSU timeout expires. That
    // is the whole reason it precedes the progress clear rather than following it. Unconditional,
    // because ending an update that never began is a no-op — and `panic = "abort"` means no unwind
    // will ever emit the closing marker on our behalf.
    let _ = out.execute(EndSynchronizedUpdate);
    // Pi's `TUI.stop()` writes `CSI ? 2031 l` before anything else of its teardown
    // (`tui.ts:972-975`): a terminal that keeps reporting appearance changes to a process that is
    // gone would type `997;1n` into the user's shell. Idempotent, and a no-op for a session that
    // never asked for the reports.
    crate::color_scheme::terminal_stopped(out);
    // The OSC 9;4 taskbar indicator next — Pi's `ProcessTerminal.stop()` clears it ahead of every
    // other teardown step (`tui/src/terminal.ts:407-409`, `if (this.clearProgressInterval())`), and
    // it is the one piece of state here that OUTLIVES the process: raw mode and bracketed paste die
    // with the tty settings a `reset` restores, but a progress indicator this process lit stays lit
    // in the taskbar until something explicitly clears it. Gated on
    // [`crate::progress_is_armed`] so a session that never armed progress emits nothing.
    if crate::terminal_progress::progress_is_armed() {
        crate::terminal_progress::write_terminal_progress(false);
    }
    let _ = out.execute(PopKeyboardEnhancementFlags);
    let _ = out.execute(DisableBracketedPaste);
    let _ = disable_raw_mode();
    let _ = out.execute(ratatui::crossterm::cursor::Show);
    // The hook's own output and the panic message that follows both have to survive an abort, and
    // an abort does not flush.
    let _ = out.flush();
}

/// Install the panic hook, chaining the existing one.
///
/// Idempotent in effect but not in cost: calling it twice chains two restores, which is harmless
/// (the restore is itself idempotent) but pointless. [`crate::App::into_stdout`] calls it once,
/// BEFORE `enable_raw_mode`, so a panic during terminal setup — between enabling raw mode and
/// returning the `App` — is covered too.
///
/// The previous hook is invoked afterwards rather than replaced, so the panic message, location and
/// any `RUST_BACKTRACE` output still reach the user. Restoring first is what makes that message
/// legible: printed under raw mode it would render as a staircase with no carriage returns.
pub fn install_panic_hook() {
    INSTALLS.fetch_add(1, Ordering::Relaxed);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // pi's `uncaughtCrash` (`interactive-mode.ts:4198-4210` @v0.87.1), in its order: unregister
        // the dead-terminal handler, so a crash on a dead terminal still ends as a crash rather than
        // as `emergencyTerminalExit`; kill the tracked detached bash groups, which would otherwise
        // outlive the process; then restore. The drain holds its registry lock only to take the
        // set, and nothing under that lock can panic, so it cannot be re-entered from here.
        crate::dead_terminal::disarm();
        cyrup_tools::kill_tracked_detached_children();
        restore_terminal_best_effort();
        previous(info);
    }));
}

/// Number of [`install_panic_hook`] calls so far.
///
/// The installed hook is process-global state that no in-process test can read back —
/// `std::panic::take_hook` hands back an opaque `Box<dyn Fn>` and `PanicHookInfo` cannot be
/// constructed to invoke it — so this counter is the only handle a test has on "did the wiring
/// actually happen". One relaxed increment on a once-per-process call.
static INSTALLS: AtomicUsize = AtomicUsize::new(0);

/// Read [`INSTALLS`]. Test-only; see that static for why it exists.
#[cfg(test)]
pub(crate) fn install_count() -> usize {
    INSTALLS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Both tests below mutate the one process-global panic hook, and libtest runs tests on
    /// parallel threads. Without this the install-count assertion would race the other test's
    /// `install_panic_hook` call and fail on an unrelated increment.
    static HOOK_LOCK: Mutex<()> = Mutex::new(());

    /// Take [`HOOK_LOCK`], ignoring poisoning: a sibling test that panicked already reported its own
    /// failure, and refusing the lock here would turn that into a second, misleading one.
    fn lock_hook() -> std::sync::MutexGuard<'static, ()> {
        HOOK_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The chaining property: the panic message must still be produced. A hook that restored the
    /// terminal and swallowed the diagnostic would trade one silent failure for another.
    #[test]
    fn the_previous_hook_still_runs_after_restoration() {
        let _guard = lock_hook();
        // The chained hook runs [`restore_terminal_best_effort`], which CLEARS the process-global
        // `PROGRESS_ARMED` — see [`crate::terminal_progress::lock_progress_armed`] for the
        // cross-module race that closes. Taken after `HOOK_LOCK`, always in that order, so the two
        // locks cannot cycle.
        let _armed = crate::terminal_progress::lock_progress_armed();
        let seen = Arc::new(AtomicUsize::new(0));
        let flag = Arc::clone(&seen);

        // Stand in for the default hook, then chain ours on top of it.
        std::panic::set_hook(Box::new(move |_| {
            flag.fetch_add(1, Ordering::SeqCst);
        }));
        install_panic_hook();

        let result = std::panic::catch_unwind(|| panic!("boom"));
        assert!(result.is_err(), "the panic still propagates");
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "the pre-existing hook must still fire, so the panic message is not swallowed"
        );

        let _ = std::panic::take_hook();
    }

    /// pi's `TUI.stop()` takes mode 2031 down (`tui.ts:972-975`) and the panic hook runs the same
    /// teardown, so a terminal left reporting appearance changes to a dead process is impossible:
    /// the escape goes out after the synchronized update is closed and before any other mode is
    /// touched.
    ///
    /// FAILS without the change: the teardown wrote no `2031` sequence at all.
    #[test]
    fn restoration_turns_the_colour_scheme_notifications_off() {
        let _scheme = crate::color_scheme::lock_for_test();
        let _armed = crate::terminal_progress::lock_progress_armed();
        crate::color_scheme::reset_for_test();
        crate::color_scheme::arm_for_test();

        let mut out = Vec::new();
        restore_into(&mut out);
        let text = String::from_utf8_lossy(&out).into_owned();
        let sync = text.find("\x1b[?2026l").expect("the update is closed");
        let off = text.find("\x1b[?2031l").expect("the mode is switched off");
        let bracketed = text.find("\x1b[?2004l").expect("bracketed paste is off");
        assert!(sync < off && off < bracketed, "{text:?}");

        // Idempotent: a second teardown has nothing left to switch off.
        let mut again = Vec::new();
        restore_into(&mut again);
        assert!(!String::from_utf8_lossy(&again).contains("2031"));
        crate::color_scheme::reset_for_test();
    }

    /// Restoration runs on a process with no TTY (CI, a piped run) without panicking itself — a
    /// hook that panicked would abort while handling an abort.
    #[test]
    fn restoration_is_safe_without_a_tty_and_is_idempotent() {
        // Same reason as above: each call clears `PROGRESS_ARMED` when it is set, and
        // `terminal_progress`'s own tests assert on that bit from a sibling thread.
        let _armed = crate::terminal_progress::lock_progress_armed();
        restore_terminal_best_effort();
        restore_terminal_best_effort();
    }

    /// The wiring: a module that is never called restores nothing. [`crate::App::into_stdout`] must
    /// install the hook, and must do it BEFORE `enable_raw_mode`.
    ///
    /// The ordering half is what makes this assertion sharp rather than incidental. With no
    /// controlling terminal `enable_raw_mode` fails and `into_stdout` returns `Err` at that line, so
    /// an install placed after it would never run — the count would not move and this test would
    /// fail. The two assertions therefore pin both "installed" and "installed first".
    ///
    /// Skipped where a controlling terminal exists (a developer running `cargo test` from a shell):
    /// there `into_stdout` would succeed, really putting that terminal into raw mode and really
    /// allocating an inline viewport underneath the test harness's output.
    #[test]
    fn into_stdout_installs_the_hook_before_it_enables_raw_mode() {
        if has_controlling_terminal() {
            return;
        }
        let _guard = lock_hook();
        let before = install_count();
        let app = crate::App::into_stdout(crate::UiTheme::default());
        assert!(
            app.is_err(),
            "precondition: with no controlling terminal enable_raw_mode must fail, which is what \
             makes the count below an ordering assertion"
        );
        assert_eq!(
            install_count(),
            before + 1,
            "App::into_stdout must call install_panic_hook() before enable_raw_mode()"
        );
        // TUI-S02 — and it registers the dead-terminal handler, pi's `registerSignalHandlers`
        // (`interactive-mode.ts:4252-4261` @v0.87.1).
        assert!(
            crate::dead_terminal::is_armed(),
            "App::into_stdout must arm the dead-terminal handler"
        );
        crate::dead_terminal::disarm();
    }

    /// pi's `uncaughtCrash` kills the tracked detached bash groups before it restores
    /// (`interactive-mode.ts:4206-4208` @v0.87.1): a crash must not leave a `sleep` the bash tool
    /// started running after the process is gone.
    #[cfg(unix)]
    #[test]
    fn a_panic_kills_the_tracked_detached_children() {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
        let _guard = lock_hook();
        let _armed = crate::terminal_progress::lock_progress_armed();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        cyrup_tools::track_detached_child_pid(child.id());
        std::panic::set_hook(Box::new(|_| {}));
        install_panic_hook();
        let result = std::panic::catch_unwind(|| panic!("boom"));
        let _ = std::panic::take_hook();
        assert!(result.is_err());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("the tracked detached child outlived the crash");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(status.signal(), Some(9), "{status:?}");
    }

    /// Mirror of the terminal crossterm would grab: stdin when it is a TTY, else `/dev/tty`.
    fn has_controlling_terminal() -> bool {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal() || std::fs::File::open("/dev/tty").is_ok()
    }
}

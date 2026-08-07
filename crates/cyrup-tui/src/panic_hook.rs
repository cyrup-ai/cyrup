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

use std::io::{self, Write};

use ratatui::crossterm::ExecutableCommand;
use ratatui::crossterm::event::{DisableBracketedPaste, PopKeyboardEnhancementFlags};
use ratatui::crossterm::terminal::disable_raw_mode;

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
pub fn restore_terminal_best_effort() {
    let mut out = io::stdout();
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
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal_best_effort();
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// The chaining property: the panic message must still be produced. A hook that restored the
    /// terminal and swallowed the diagnostic would trade one silent failure for another.
    #[test]
    fn the_previous_hook_still_runs_after_restoration() {
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

    /// Restoration runs on a process with no TTY (CI, a piped run) without panicking itself — a
    /// hook that panicked would abort while handling an abort.
    #[test]
    fn restoration_is_safe_without_a_tty_and_is_idempotent() {
        restore_terminal_best_effort();
        restore_terminal_best_effort();
    }
}

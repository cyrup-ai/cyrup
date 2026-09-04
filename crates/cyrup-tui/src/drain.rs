//! Draining stdin before the terminal goes back to cooked mode — the port of Pi's
//! `Terminal.drainInput` (`tui/src/terminal.ts:368-404`, declared on the interface at `:65`) and of
//! the `process.stdin.pause()` that closes `Terminal.stop` (`:446`).
//!
//! # The failure this prevents
//!
//! At exit the app pops the Kitty keyboard flags, disables bracketed paste and leaves raw mode
//! ([`crate::App::restore`]). Anything still sitting in the terminal's input queue at the moment raw
//! mode goes away is **not** discarded — it is handed to the next reader of that tty, which is the
//! user's shell. Two things are routinely in flight there:
//!
//! * **Kitty key-*release* events.** With `DISAMBIGUATE_ESCAPE_CODES` pushed, the final `Ctrl+D` /
//!   `Ctrl+C` that asked for the quit also generates a release report. Over a slow SSH link that
//!   report is still on the wire while the process is already tearing the terminal down, so it
//!   lands in cooked mode and the shell echoes a bare `[109;5u` (or similar) at the prompt.
//! * **The keypress itself.** Pi's own comment at `terminal.ts:441-445` names the concrete
//!   consequence: a buffered `Ctrl+D` re-interpreted after raw mode is off closes the parent shell,
//!   i.e. drops an SSH session.
//!
//! Neither is hypothetical and neither is recoverable from inside cyrup once the bytes are gone.
//!
//! # The port
//!
//! Pi's loop is `while (timeLeft > 0 && now - lastDataTime < idleMs) await sleep(min(idleMs,
//! timeLeft))`, with a `data` listener attached to a *flowing* stdin — so the wait both consumes the
//! bytes and refreshes `lastDataTime`. [`drain_input`] is that loop with the sleep replaced by a
//! bounded read ([`InputDrain::consume_ready`]), which is the same thing expressed synchronously:
//! wait up to one idle window, consume whatever showed up, and let any arrival restart the idle
//! countdown. It stops on the first idle window that stays quiet, and never runs past `max`.
//!
//! Pi disables the Kitty protocol *first*, before waiting, "so any late key releases do not generate
//! new Kitty escape sequences" (`:370-377`) — otherwise the drain would race a source that is still
//! producing. [`drain_stdin_before_exit`] keeps that ordering.
//!
//! `process.stdin.pause()` has no direct Rust analog — cyrup has no flowing stdin stream to pause,
//! and the crossterm reader thread it *does* have is detached and unstoppable. The property that
//! call buys Pi (nothing buffered gets re-read after raw mode is off) is bought here by the drain
//! itself, which is why the drain must run BEFORE [`crate::App::restore`] rather than after it.
//!
//! # Not stealing input, not hanging
//!
//! This module reads from the same fd as [`crate::terminal_query`] and inherits its contract: every
//! read is `poll(2)`-bounded, nothing blocks past the deadline, and the whole thing is skipped
//! outright unless stdin/stdout are a tty and raw mode is still on. The difference is intent — the
//! probe module reads a *reply* and must not touch a keystroke, whereas this one deliberately
//! discards everything it finds, which is only correct on the exit path where "everything" is by
//! definition input the app will never act on. It is therefore called from exactly one place
//! ([`crate::App::drain_and_restore`]) and NOT from [`crate::App::restore`], which also runs on the
//! suspend (Ctrl+Z) and external-editor paths where the user's typing must survive — Pi draws the
//! same line, calling `drainInput` only from `shutdown()` (`interactive-mode.ts:3578`, `:3589`) and
//! never from `handleCtrlZ` (`:3722`, a bare `ui.stop()`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Pi's `maxMs` default, and the value both of its call sites pass explicitly
/// (`interactive-mode.ts:3578`, `:3589` — `drainInput(1000)`).
pub const DRAIN_MAX: Duration = Duration::from_millis(1000);

/// Pi's `idleMs` default (`terminal.ts:368`): the quiet window that ends the drain.
pub const DRAIN_IDLE: Duration = Duration::from_millis(50);

/// The "is anything still arriving?" half of the drain, behind a trait so [`drain_input`]'s loop is
/// exercised off a real tty. Pi's equivalent seam is its `data` listener plus `setTimeout`.
pub trait InputDrain {
    /// Wait up to `timeout` for readable input, consume everything that is ready, and report how
    /// many bytes went away. `0` means the wait expired with nothing to read — the quiet window Pi's
    /// `now - lastDataTime >= idleMs` is testing for.
    ///
    /// Two obligations, and [`drain_input`] is correct only if both hold:
    ///
    /// * **Never block past `timeout`.** It is the only thing bounding the loop between deadline
    ///   checks.
    /// * **When nothing arrives, consume the WHOLE `timeout`.** Returning `0` early would spin the
    ///   loop instead of advancing the idle clock, and the drain would burn CPU until `max`. The
    ///   real implementation gets this from `poll(2)`, which blocks for the full interval before
    ///   reporting a timeout; a stand-in has to sleep.
    fn consume_ready(&mut self, timeout: Duration) -> usize;
}

/// Pi `Terminal.drainInput(maxMs, idleMs)` (`terminal.ts:368-404`), returning the number of bytes
/// discarded.
///
/// Terminates after the first `idle` window in which nothing arrived, or when `max` is exhausted —
/// whichever comes first. Every arrival restarts the idle countdown, so a burst that keeps coming
/// (a slow link flushing a paste) is drained up to the `max` budget instead of half-drained.
///
/// Like Pi's, the loop always waits at least one idle window before concluding the queue is quiet:
/// the bytes this exists to catch are precisely the ones that have not landed yet.
pub fn drain_input<D: InputDrain + ?Sized>(source: &mut D, max: Duration, idle: Duration) -> usize {
    let start = Instant::now();
    let mut last_data = start;
    let mut drained = 0usize;
    loop {
        let now = Instant::now();
        // Pi's `timeLeft = endTime - now; if (timeLeft <= 0) break`.
        let elapsed = now.saturating_duration_since(start);
        if elapsed >= max {
            break;
        }
        let time_left = max - elapsed;
        // Pi's `if (now - lastDataTime >= idleMs) break` — the queue has gone quiet.
        if now.saturating_duration_since(last_data) >= idle {
            break;
        }
        // Pi's `await sleep(Math.min(idleMs, timeLeft))`, doing the consuming that its flowing
        // stdin does in the background.
        let n = source.consume_ready(idle.min(time_left));
        if n > 0 {
            drained = drained.saturating_add(n);
            last_data = Instant::now();
        }
    }
    drained
}

/// Number of [`drain_stdin_before_exit`] calls so far.
///
/// The drain's real effect — bytes removed from a tty input queue — is unobservable from a test
/// process that has no tty, so this counter is the only handle a test has on "the exit path actually
/// called it". Same device, and same reason, as [`crate::panic_hook`]'s install counter.
static DRAINS: AtomicUsize = AtomicUsize::new(0);

/// Read [`DRAINS`]. See that static for why it exists.
pub fn drain_count() -> usize {
    DRAINS.load(Ordering::Relaxed)
}

/// The whole exit-path sequence Pi runs ahead of `stop()`: disable the Kitty keyboard protocol so
/// nothing new is produced, then drain what is already queued. Returns the bytes discarded.
///
/// A no-op returning `0` when stdin/stdout are not a tty or raw mode is already off — in cooked mode
/// there is nothing to protect the shell from and a read would block on the line discipline.
pub fn drain_stdin_before_exit() -> usize {
    DRAINS.fetch_add(1, Ordering::Relaxed);
    if !stdin_is_drainable() {
        return 0;
    }
    // Pi `terminal.ts:370-377`: `\x1b[<u` BEFORE the wait. crossterm's `PopKeyboardEnhancementFlags`
    // is that sequence. `App::restore` pops again a moment later; the Kitty spec makes a pop against
    // an empty stack a no-op, and keeping `restore` total (it is also the panic-path teardown) is
    // worth more than eliding the duplicate.
    use ratatui::crossterm::ExecutableCommand;
    let _ = std::io::stdout().execute(ratatui::crossterm::event::PopKeyboardEnhancementFlags);
    drain_input(&mut StdinDrain, DRAIN_MAX, DRAIN_IDLE)
}

/// Both preconditions for draining: stdin/stdout are a terminal, and raw mode is still on. Mirrors
/// [`crate::terminal_query`]'s `stdin_is_queryable` — in cooked mode a read blocks until the user
/// presses Enter, which is the exact hang this must never introduce.
fn stdin_is_drainable() -> bool {
    use ratatui::crossterm::terminal::is_raw_mode_enabled;
    use ratatui::crossterm::tty::IsTty;
    std::io::stdin().is_tty()
        && std::io::stdout().is_tty()
        && is_raw_mode_enabled().unwrap_or(false)
}

/// The production [`InputDrain`]: `poll(2)` the real stdin fd for readiness, then read and throw the
/// bytes away.
struct StdinDrain;

#[cfg(unix)]
impl InputDrain for StdinDrain {
    fn consume_ready(&mut self, timeout: Duration) -> usize {
        use rustix::event::{PollFd, PollFlags, Timespec};

        let stdin = std::io::stdin();
        let ts = Timespec {
            tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX) as _,
            tv_nsec: i64::from(timeout.subsec_nanos()) as _,
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&ts)) {
            // Nothing arrived inside the window — Pi's quiet `idleMs`.
            Ok(0) => return 0,
            Ok(_) => {}
            // An interrupted (`EINTR`) or failed poll is reported as "quiet": retrying here could
            // outlive the caller's budget, and the caller's deadline check must stay authoritative.
            Err(_) => return 0,
        }
        let mut chunk = [0u8; 256];
        // A failed read (including `EINTR`/`EAGAIN`) is reported as "nothing arrived", for the same
        // reason the poll above is: the caller's deadline stays authoritative.
        rustix::io::read(&stdin, &mut chunk[..]).unwrap_or(0)
    }
}

/// Non-Unix has no `poll`-able stdin fd here (same limitation as [`crate::terminal_query`]'s
/// `read_reply`); the drain degrades to "nothing to do".
#[cfg(not(unix))]
impl InputDrain for StdinDrain {
    fn consume_ready(&mut self, _timeout: Duration) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::sync::Mutex;

    use super::*;

    /// [`DRAINS`] is process-global and libtest runs tests in parallel, so every test that reads the
    /// counter — or moves it — has to hold this first, or one test's increment lands inside another's
    /// before/after window. Same device as [`crate::panic_hook`]'s `HOOK_LOCK`.
    static DRAIN_LOCK: Mutex<()> = Mutex::new(());

    /// Take [`DRAIN_LOCK`], ignoring poisoning: a sibling that panicked has already reported its own
    /// failure, and refusing the lock here would turn that into a second, misleading one.
    fn lock_drains() -> std::sync::MutexGuard<'static, ()> {
        DRAIN_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A scripted queue: each `consume_ready` hands back the next chunk size, `0` once the script is
    /// exhausted. Records the timeouts it was asked to wait for so the loop's budget arithmetic is
    /// observable without a clock.
    ///
    /// The empty case **sleeps for the whole timeout**, which is the [`InputDrain`] contract and what
    /// `poll(2)` really does. That is also what makes the `timeouts.len()` assertions below immune to
    /// CPU contention rather than timing-dependent: the sleep guarantees at least `timeout` has
    /// passed, so the loop's next idle/deadline check is guaranteed to break. A fake that returned
    /// `0` instantly would let the loop spin an unbounded number of times and the count would depend
    /// on how loaded the box is.
    struct Scripted {
        chunks: std::collections::VecDeque<usize>,
        timeouts: Vec<Duration>,
    }

    impl Scripted {
        fn new(chunks: &[usize]) -> Self {
            Scripted {
                chunks: chunks.iter().copied().collect(),
                timeouts: Vec::new(),
            }
        }
    }

    impl InputDrain for Scripted {
        fn consume_ready(&mut self, timeout: Duration) -> usize {
            self.timeouts.push(timeout);
            match self.chunks.pop_front() {
                // Data was already waiting: a real `poll` returns at once.
                Some(n) => n,
                // Nothing there: block for the full window, as `poll` does on a timeout.
                None => {
                    std::thread::sleep(timeout);
                    0
                }
            }
        }
    }

    /// The whole point: bytes already queued when the app decides to exit are consumed here rather
    /// than surfacing at the parent shell's prompt.
    #[test]
    fn buffered_bytes_are_consumed_rather_than_left_for_the_shell() {
        let mut source = Scripted::new(&[6, 3, 12]);
        let drained = drain_input(&mut source, DRAIN_MAX, DRAIN_IDLE);
        assert_eq!(
            drained, 21,
            "every queued byte must be drained before raw mode is dropped, not just the first chunk"
        );
    }

    /// A queue that never goes quiet must not be drained forever: `max` is the hard stop, so a
    /// terminal spewing input cannot wedge the exit path.
    #[test]
    fn a_never_quiet_queue_still_terminates_at_the_max_budget() {
        /// Always ready, always one byte — Pi's `lastDataTime` would be refreshed on every tick.
        struct Endless;
        impl InputDrain for Endless {
            fn consume_ready(&mut self, timeout: Duration) -> usize {
                std::thread::sleep(timeout);
                1
            }
        }
        let started = Instant::now();
        let drained = drain_input(
            &mut Endless,
            Duration::from_millis(120),
            Duration::from_millis(20),
        );
        assert!(
            drained > 0,
            "an endlessly-ready source is drained, not skipped"
        );
        // Only an upper bound is asserted, and a very loose one: under contention the sleeps
        // overshoot arbitrarily, but the loop must never keep *starting* new waits past `max`.
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "drain_input must stop starting new waits once `max` is exhausted (elapsed {:?})",
            started.elapsed()
        );
    }

    /// A quiet queue costs one idle window and no more — the drain must not add a full second to
    /// every exit.
    #[test]
    fn a_quiet_queue_ends_after_a_single_idle_window() {
        let mut source = Scripted::new(&[]);
        let drained = drain_input(&mut source, DRAIN_MAX, DRAIN_IDLE);
        assert_eq!(drained, 0);
        assert_eq!(
            source.timeouts.len(),
            1,
            "one quiet idle window is enough to conclude the queue is empty"
        );
        assert_eq!(
            source.timeouts[0], DRAIN_IDLE,
            "the first wait is a whole idle window (Pi's `min(idleMs, timeLeft)`)"
        );
    }

    /// `max` clamps the wait when less than an idle window is left (Pi's `Math.min(idleMs,
    /// timeLeft)`), so the drain cannot overshoot its budget by up to a whole idle window.
    #[test]
    fn the_wait_is_clamped_by_the_remaining_budget() {
        let mut source = Scripted::new(&[]);
        let max = Duration::from_millis(5);
        drain_input(&mut source, max, Duration::from_millis(500));
        assert_eq!(source.timeouts.len(), 1);
        assert!(
            source.timeouts[0] <= max,
            "a wait longer than the remaining budget would blow past `max`: {:?}",
            source.timeouts[0]
        );
    }

    /// A zero budget does nothing at all — no wait is even started.
    #[test]
    fn a_zero_budget_reads_nothing() {
        let mut source = Scripted::new(&[9]);
        assert_eq!(drain_input(&mut source, Duration::ZERO, DRAIN_IDLE), 0);
        assert!(source.timeouts.is_empty());
    }

    /// The live entry point is safe on a process with no tty (CI, a piped run): it must return
    /// immediately having consumed nothing, never block on the line discipline.
    #[test]
    fn the_live_drain_is_a_bounded_no_op_without_a_raw_tty() {
        let _guard = lock_drains();
        let started = Instant::now();
        assert_eq!(drain_stdin_before_exit(), 0);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the no-tty path must not wait on anything"
        );
    }

    /// The wiring, and the half of it that is easy to get wrong.
    ///
    /// A drain nobody calls drains nothing, so this pins the pairing the exit path depends on:
    /// [`crate::App::drain_and_restore`] — the teardown `crates/cyrup/src/main.rs` runs when the
    /// interactive loop returns — must go through [`drain_stdin_before_exit`], and the plain
    /// [`crate::App::restore`] must NOT. That second assertion is the load-bearing one: `restore`
    /// also runs on Ctrl+Z and around the external editor, where discarding the user's buffered
    /// typing would be a fresh bug, and Pi is equally careful (`handleCtrlZ` calls a bare
    /// `ui.stop()`, `interactive-mode.ts:3722`).
    ///
    /// Driven over a `TestBackend` because a `CrosstermBackend<Stdout>` needs a controlling terminal
    /// to size itself; neither method touches the backend beyond `show_cursor`, so the pairing under
    /// test is identical to the production one.
    #[test]
    fn drain_and_restore_drains_but_plain_restore_leaves_input_alone() {
        let _guard = lock_drains();
        let mut app = crate::App::new(
            ratatui::backend::TestBackend::new(40, 8),
            crate::UiTheme::default(),
        )
        .expect("a TestBackend app");

        let before = drain_count();
        let _ = app.restore();
        assert_eq!(
            drain_count(),
            before,
            "App::restore is the suspend / external-editor teardown — it must not eat input the \
             user still owns"
        );

        let _ = app.drain_and_restore();
        assert_eq!(
            drain_count(),
            before + 1,
            "App::drain_and_restore must run Pi's `drainInput` before restoring the terminal \
             (interactive-mode.ts:3589-3591)"
        );
    }
}

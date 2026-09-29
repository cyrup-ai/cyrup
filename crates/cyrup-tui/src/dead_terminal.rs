//! The dead-terminal emergency exit — cyrup's port of pi's `DEAD_TERMINAL_ERROR_CODES` →
//! `emergencyTerminalExit` (TUI-S02).
//!
//! # What upstream does
//!
//! `registerSignalHandlers` (`interactive-mode.ts:4252-4261` @v0.87.1) installs one
//! `terminalErrorHandler` on BOTH `process.stdout` and `process.stderr`. An error whose `code` is
//! `EIO`, `EPIPE` or `ENOTCONN` (`isDeadTerminalError`, `:264-272`) — the terminal hung up, its
//! pane was killed, the SSH link dropped — goes to `emergencyTerminalExit` (`:4179-4186`):
//!
//! ```ts
//! this.isShuttingDown = true;
//! this.unregisterSignalHandlers();
//! killTrackedDetachedChildren();
//! // The terminal is gone. Do not run normal shutdown because TUI and
//! // extension cleanup can write restore sequences and re-trigger EIO.
//! process.exit(129);
//! ```
//!
//! Any other stream error is rethrown. The handler has no precondition beyond being registered,
//! and it stays registered for the whole interactive session — including through a graceful
//! shutdown, whose restore writes are exactly how a dead terminal is detected after a `SIGHUP`
//! (`:4238-4245`). The one path that removes it first is `uncaughtCrash` (`:4198-4205`), which
//! unregisters before its own restore.
//!
//! # How cyrup maps it
//!
//! Node reports a stream error as an event on the stream; Rust reports it as the `Err` of the write
//! that failed. So the handler lives on the writer: [`TerminalWriter`] wraps every handle the TUI
//! writes the terminal through — [`terminal_stdout`] for the mode, title and progress writes,
//! [`crate::write_log::TuiStdout`] (which sits on top of it) for frames and the alternate screen,
//! and [`terminal_stderr`] for the diagnostics printed while the session is live. A failed write or
//! flush is classified by [`is_dead_terminal_error`]; while the handler is [`arm`]ed a match calls
//! [`emergency_terminal_exit`], which writes nothing and exits 129, and anything else is returned to
//! the caller unchanged. [`crate::App::into_stdout`] arms it — pi's `registerSignalHandlers` runs
//! in interactive-mode `init()` — and the panic hook disarms it before restoring, as `uncaughtCrash`
//! unregisters (and, like `uncaughtCrash`, kills the tracked detached children before it
//! restores).
//!
//! 129 is `128 + SIGHUP`, the code a hung-up terminal's signal would have produced, and the same
//! one cyrup's signal handling gives `SIGHUP` (`crates/cyrup/src/signals.rs`, SEAM-008).

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// pi's `emergencyTerminalExit` status (`interactive-mode.ts:4185`).
pub const DEAD_TERMINAL_EXIT_CODE: i32 = 129;

/// Whether the handler is registered.
static ARMED: AtomicBool = AtomicBool::new(false);

/// Register the handler — pi's `process.stdout.on("error", …)` / `process.stderr.on("error", …)`.
pub(crate) fn arm() {
    ARMED.store(true, Ordering::SeqCst);
}

/// Remove it — pi's `unregisterSignalHandlers()` (`interactive-mode.ts:4271-4276`).
pub(crate) fn disarm() {
    ARMED.store(false, Ordering::SeqCst);
}

/// Whether the handler is registered — what a wiring test reads back after [`arm`]'s caller ran.
#[cfg(test)]
pub(crate) fn is_armed() -> bool {
    ARMED.load(Ordering::SeqCst)
}

/// pi `isDeadTerminalError` (`interactive-mode.ts:264-272`) over `DEAD_TERMINAL_ERROR_CODES =
/// ["EIO", "EPIPE", "ENOTCONN"]`. std names two of the three as kinds on every platform; `EIO` has
/// no kind and is read off the raw errno.
pub(crate) fn is_dead_terminal_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected
    ) || is_eio(e)
}

#[cfg(unix)]
fn is_eio(e: &io::Error) -> bool {
    e.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error())
}

#[cfg(not(unix))]
fn is_eio(_: &io::Error) -> bool {
    false
}

/// pi `emergencyTerminalExit`: stop listening, kill the tracked detached bash groups, and leave
/// with 129 — writing nothing, because every restore sequence would hit the dead terminal again.
fn emergency_terminal_exit() -> ! {
    disarm();
    cyrup_tools::kill_tracked_detached_children();
    std::process::exit(DEAD_TERMINAL_EXIT_CODE)
}

/// The armed handler's decision for one failed write.
fn on_write_error(e: &io::Error) {
    if ARMED.load(Ordering::SeqCst) && is_dead_terminal_error(e) {
        emergency_terminal_exit();
    }
}

/// A terminal handle whose failed writes go through pi's `terminalErrorHandler`.
#[derive(Debug)]
pub struct TerminalWriter<W>(W);

impl<W: Write> Write for TerminalWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf).inspect_err(on_write_error)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush().inspect_err(on_write_error)
    }
}

/// Stdout, as the TUI writes it.
pub fn terminal_stdout() -> TerminalWriter<io::Stdout> {
    TerminalWriter(io::stdout())
}

/// Stderr, as the interactive session writes it — pi's second `terminalErrorHandler` site.
pub fn terminal_stderr() -> TerminalWriter<io::Stderr> {
    TerminalWriter(io::stderr())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::io::Read as _;
    use std::process::{Command, Stdio};

    use super::*;

    /// A writer that fails every call with `kind`.
    struct Failing(io::ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.0))
        }
    }

    #[test]
    fn the_three_dead_terminal_codes_and_nothing_else() {
        assert!(is_dead_terminal_error(&io::Error::from(
            io::ErrorKind::BrokenPipe
        )));
        assert!(is_dead_terminal_error(&io::Error::from(
            io::ErrorKind::NotConnected
        )));
        #[cfg(unix)]
        assert!(is_dead_terminal_error(&io::Error::from_raw_os_error(
            rustix::io::Errno::IO.raw_os_error()
        )));
        assert!(!is_dead_terminal_error(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
        assert!(!is_dead_terminal_error(&io::Error::from(
            io::ErrorKind::Interrupted
        )));
    }

    /// pi's handler rethrows any other error (`throw error`): an armed writer hands a
    /// `PermissionDenied` back to its caller and the process carries on.
    #[test]
    fn any_other_write_error_is_returned_not_fatal() {
        arm();
        let mut out = TerminalWriter(Failing(io::ErrorKind::PermissionDenied));
        let err = out.write_all(b"\x1b[?25h").unwrap_err();
        disarm();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    /// With the handler not registered — before interactive mode, or after the panic hook
    /// unregistered it — a dead terminal is an ordinary error.
    #[test]
    fn a_disarmed_writer_returns_the_dead_terminal_error() {
        disarm();
        let mut out = TerminalWriter(Failing(io::ErrorKind::BrokenPipe));
        assert_eq!(out.flush().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    /// Set on the re-executed child of
    /// [`a_write_to_a_hung_up_stdout_exits_129_and_writes_nothing_more`]: arm, wait until the
    /// parent has hung up the read end of our stdout, then write a frame through the TUI's channel
    /// and — if that somehow returned — a line to stderr.
    const CHILD_ENV: &str = "CYRUP_TUI_DEAD_TERMINAL_CHILD";

    /// **TUI-S02 `Verify`.** A real dead stdout — a pipe whose reader is gone, so the write fails
    /// with `EPIPE` — driven through the production write channel of an armed session: the process
    /// exits 129 at the first failing write and runs nothing after it (the child's fallthrough
    /// status 3 and its stderr marker never appear).
    #[test]
    fn a_write_to_a_hung_up_stdout_exits_129_and_writes_nothing_more() {
        if std::env::var_os(CHILD_ENV).is_some() {
            arm();
            // Tell the parent the harness's own stdout chatter is done, then wait for the hang-up.
            let mut stdout = io::stdout();
            let _ = stdout.write_all(b"ready\n");
            let _ = stdout.flush();
            let mut byte = [0u8; 1];
            let _ = io::stdin().read(&mut byte);
            let mut frame = crate::write_log::tui_stdout();
            let _ = frame.write_all(b"frame\n");
            let _ = frame.flush();
            let _ = terminal_stderr().write_all(b"survived the dead terminal\n");
            std::process::exit(3);
        }
        let exe = std::env::current_exe().unwrap();
        let mut child = Command::new(exe)
            .args([
                "--exact",
                "dead_terminal::tests::a_write_to_a_hung_up_stdout_exits_129_and_writes_nothing_more",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while io::BufRead::read_line(&mut stdout, &mut line).unwrap() > 0 && line != "ready\n" {
            line.clear();
        }
        // Hang up: the child's stdout now has no reader, so its next write fails with `EPIPE`.
        drop(stdout);
        child.stdin.take().unwrap().write_all(b"go").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(DEAD_TERMINAL_EXIT_CODE),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("survived"),
            "nothing runs after the emergency exit"
        );
    }
}

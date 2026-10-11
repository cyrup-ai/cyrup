//! SEAM-161 (a) — the one call in `main.rs` that sends `tracing` to `<agent dir>/logs/cyrup.log`
//! when the interactive UI owns the terminal (`c809adfcf`, `PERM-042`).
//!
//! The rule (`bootstrap::should_redirect_tracing`) and the writer are unit-tested in
//! `bootstrap.rs`; the call that applies them was pinned only by a live tmux run. These tests run
//! the real binary on a real pseudo-terminal (`openpty`), which is the one input the rule reads
//! that a pipe cannot fake: `stderr.is_terminal()`.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Read as _;
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A fresh pseudo-terminal: (controller, terminal).
fn open_pty() -> (OwnedFd, OwnedFd) {
    let mut controller: libc::c_int = -1;
    let mut terminal: libc::c_int = -1;
    // SAFETY: `openpty` writes two fresh descriptors into the out-pointers on success and reads
    // nothing through the three null arguments (no name buffer, default termios and window size).
    let rc = unsafe {
        libc::openpty(
            &raw mut controller,
            &raw mut terminal,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: both descriptors were just opened by `openpty` and are owned by nothing else.
    unsafe {
        (
            OwnedFd::from_raw_fd(controller),
            OwnedFd::from_raw_fd(terminal),
        )
    }
}

/// A `cyrup` with no arguments (the interactive mode), isolated from the user's own directories
/// and tracing at the finest level, so events follow the redirect at once.
fn interactive(tmp: &Path) -> Command {
    let home = tmp.join("home");
    let project = tmp.join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.current_dir(&project)
        .env("HOME", &home)
        .env("CYRUP_AGENT_DIR", tmp.join("agent"))
        .env("RUST_LOG", "trace")
        .env("TERM", "xterm-256color");
    cmd
}

fn log_file(tmp: &Path) -> PathBuf {
    tmp.join("agent").join("logs").join("cyrup.log")
}

/// Wait up to `bound` for the trace log to hold something.
fn wait_for_log(path: &Path, bound: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < bound {
        if std::fs::metadata(path).is_ok_and(|meta| meta.len() > 0) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// stdin, stdout and stderr on one terminal: the interactive UI owns it, so `tracing` goes to the
/// log. Passes at HEAD: SEAM-161 is a test-defect row, and this is the pin it asked for — a
/// NON-REGRESSION GUARD for the `main.rs` call. Removing the call turns it red (no log file after
/// the 30 s bound).
#[test]
fn an_interactive_session_on_a_terminal_writes_its_trace_to_the_log_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (controller, terminal) = open_pty();
    let mut cmd = interactive(tmp.path());
    cmd.stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal.try_clone().unwrap()))
        .stderr(Stdio::from(terminal));
    let child = cmd.spawn().expect("spawn cyrup on the pty");
    // The command holds its own copies of the terminal; only the child may keep it open.
    drop(cmd);
    // Drain the terminal, so the UI never blocks on a full pty buffer. Ends with an error when the
    // child is gone and the last terminal descriptor closes; not joined, so a grandchild that kept
    // the terminal cannot hold the test.
    let mut screen = std::fs::File::from(controller);
    let drain = std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = screen.read_to_end(&mut sink);
    });

    let written = wait_for_log(&log_file(tmp.path()), Duration::from_secs(30));
    stop(child);
    drop(drain);
    assert!(
        written,
        "the interactive session on a terminal sent its trace to {}",
        log_file(tmp.path()).display()
    );
}

/// stderr redirected (not a terminal) while stdin and stdout are: the redirect is the user's
/// choice of where diagnostics go, so `tracing` stays on it and no log is written. Passes at HEAD —
/// the NON-REGRESSION GUARD for the call's condition: applying the redirect unconditionally turns it
/// red.
#[test]
fn an_interactive_session_whose_stderr_is_redirected_keeps_its_trace_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let (controller, terminal) = open_pty();
    let mut cmd = interactive(tmp.path());
    cmd.stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal))
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn cyrup on the pty");
    drop(cmd);
    let mut screen = std::fs::File::from(controller);
    let drain = std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = screen.read_to_end(&mut sink);
    });
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    // The same window the positive test needs, so a redirect would have had time to write.
    std::thread::sleep(Duration::from_secs(3));
    stop(child);
    drop(drain);
    let stderr_text = errors.join().unwrap();
    assert!(
        !log_file(tmp.path()).exists(),
        "a redirected stderr keeps the trace; no log file is written"
    );
    assert!(
        !stderr_text.is_empty(),
        "and the trace reached the redirected stderr"
    );
}

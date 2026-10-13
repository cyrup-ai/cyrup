//! CFG-115 — a broken `<agent_dir>/models.json` is reported ONCE per run, where the run shows it.
//!
//! pi prints no `models.json` error before its UI starts and shows it in the interactive
//! transcript (`interactive-mode.ts:1211-1214` @f1b2e77f5). cyrup reported it on stderr in every
//! mode before the TUI took the terminal, and SEAM-150 added the transcript line, so an interactive
//! boot showed it twice. Driven through the shipped binary in a cleared environment.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Read as _;
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BROKEN: &str = "{ not json";

/// A `cyrup` with a broken `models.json`, isolated from the user's directories.
fn cyrup(tmp: &Path) -> Command {
    let home = tmp.join("home");
    let agent = tmp.join("agent");
    let project = tmp.join("project");
    for dir in [&home, &agent, &project] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(agent.join("models.json"), BROKEN).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.current_dir(&project).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", &home)
        .env("CYRUP_AGENT_DIR", &agent)
        .env("CYRUP_OFFLINE", "1")
        .env("TERM", "xterm-256color");
    cmd
}

fn models_json_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.contains("models.json")).collect()
}

/// Print mode has no transcript, so stderr is where the error must still go.
///
/// NON-REGRESSION GUARD: green at HEAD; red under the over-fix that skips the pre-UI report in
/// every mode (`reports_models_json_pre_ui -> false`).
#[test]
fn print_mode_still_reports_a_broken_models_json_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let out = cyrup(tmp.path())
        .args(["-p", "hi"])
        .output()
        .expect("spawning the cyrup binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !models_json_lines(&stderr).is_empty(),
        "print mode reports the broken models.json on stderr:\n{stderr}"
    );
}

/// A fresh 120x40 pseudo-terminal: (controller, terminal). The size matters: a `0x0` terminal
/// (`openpty`'s default) paints an empty frame, so the transcript would never be visible.
fn open_pty() -> (OwnedFd, OwnedFd) {
    let mut controller: libc::c_int = -1;
    let mut terminal: libc::c_int = -1;
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `openpty` writes two fresh descriptors into the out-pointers on success, reads the
    // window size from `size` (a live local) and nothing through the two null arguments (no name
    // buffer, default termios).
    let rc = unsafe {
        libc::openpty(
            &raw mut controller,
            &raw mut terminal,
            std::ptr::null_mut(),
            std::ptr::null(),
            &raw const size,
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

/// Drop CSI / OSC escape sequences so the painted text can be searched.
fn strip_ansi(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Interactive mode on a real terminal, stderr redirected so it can be read on its own: the error
/// is painted in the transcript and NOT written to stderr.
///
/// RED at HEAD: `main.rs` ran `diagnostics::report(&models_json_warnings)` before the TUI in every
/// mode, so stderr carried a `Warning: … models.json …` line (also red under the mutation
/// `reports_models_json_pre_ui -> true`).
#[test]
fn interactive_mode_shows_a_broken_models_json_in_the_transcript_only() {
    let tmp = tempfile::tempdir().unwrap();
    let (controller, terminal) = open_pty();
    let mut cmd = cyrup(tmp.path());
    cmd.stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal))
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn cyrup on the pty");
    drop(cmd);

    let screen = Arc::new(Mutex::new(Vec::new()));
    let mut controller = std::fs::File::from(controller);
    let sink = Arc::clone(&screen);
    let _drain = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = controller.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let started = Instant::now();
    let mut painted = false;
    while started.elapsed() < Duration::from_secs(30) {
        if strip_ansi(&screen.lock().unwrap()).contains("models.json error") {
            painted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    let stderr_text = errors.join().unwrap();

    assert!(
        painted,
        "the transcript shows `models.json error: …`; screen:\n{}",
        strip_ansi(&screen.lock().unwrap())
    );
    assert!(
        models_json_lines(&stderr_text).is_empty(),
        "nothing about models.json reaches stderr in interactive mode:\n{stderr_text}"
    );
}

//! SEAM-047 end-to-end — the FIRST SIGTERM/SIGHUP must tear the process down, with pi's exit code.
//!
//! pi registers `["SIGTERM", "SIGHUP"]` in every non-interactive host and the handler exits on the
//! FIRST delivery: `print-mode.ts:48-64` (`disposeRuntime().finally(() => process.exit(signal ===
//! "SIGHUP" ? 129 : 143))`) and `rpc-mode.ts:365-379` → `shutdown(signal === "SIGHUP" ? 129 : 143,
//! signal)` (`:723-740`). cyrup's watcher only ran `session.abort() + cancel.cancel()` on the first
//! delivery and no cancel token reached the RPC serving loop, so a live `cyrup --mode rpc` ignored
//! the first SIGTERM *and* the first SIGHUP, was still running 15 s later, and needed SIGKILL; only
//! a SECOND delivery exited 143.
//!
//! This can only be observed against the real binary with a real signal. Fully offline, hermetic
//! tempdir HOME/agent dir, stdin held open on a pipe so RPC mode does not exit on EOF.
//!
//! MIGRATION: the file-level `#![cfg(unix)]` this carried in `crates/cyrup/tests/` is now the
//! `#[cfg(unix)]` on this module's declaration in `main.rs` — same predicate, one place, and
//! greppable from the target's inventory.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Spawn `cyrup --mode rpc` with stdin held open, in a hermetic offline tempdir.
fn spawn_rpc() -> (Child, ChildStdin, ChildStdout, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut child = Command::new(crate::support::bins::cyrup())
        .current_dir(&work)
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        // Never inherit an ambient built-in opt-in — see `unknown_flag_exit.rs`.
        .env_remove("CYRUP_INTERCOM")
        .env_remove("CYRUP_SUBAGENTS")
        .env_remove("CYRUP_PERMISSION_SYSTEM")
        .args([
            "--mode",
            "rpc",
            "--offline",
            "--no-session",
            "--no-extensions",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cyrup --mode rpc");
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    (child, stdin, stdout, tmp)
}

/// Prove the RPC serving loop is UP before signalling — a `get_state` round-trip, the same readiness
/// handshake `cyrup-modes/tests/modes.rs:881-883` uses. Without it a signal delivered during startup
/// takes the default disposition (the watcher is spawned after the runtime is built), which would
/// make this test measure process startup latency rather than the handler.
fn await_rpc_ready(stdin: &mut ChildStdin, stdout: ChildStdout) {
    stdin
        .write_all(b"{\"type\":\"get_state\",\"id\":\"ready\"}\n")
        .expect("write get_state");
    stdin.flush().expect("flush");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read a response line");
    assert!(
        line.contains("\"ready\""),
        "expected the get_state response, got: {line}"
    );
}

/// Wait up to `limit` for the child to exit; `None` means it was still running at the deadline.
fn wait_for_exit(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => return Some(status.code().unwrap_or(-1)),
            None if Instant::now() >= deadline => return None,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn kill_with(child: &Child, signal: &str) {
    let status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {signal} failed");
}

/// Drain whatever the child wrote, for failure messages.
fn drain_stderr(child: &mut Child) -> String {
    let mut buf = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut buf);
    }
    buf
}

/// Run one signal end to end: start rpc mode, prove it is alive, deliver ONE signal, require it to
/// be gone within the window with pi's exit code.
fn first_delivery_exits(signal: &str, expected_code: i32) {
    let (mut child, mut stdin, stdout, _tmp) = spawn_rpc();

    // It must actually reach the serving loop first, or "it exited" would prove nothing.
    await_rpc_ready(&mut stdin, stdout);
    assert!(
        wait_for_exit(&mut child, Duration::from_millis(100)).is_none(),
        "cyrup --mode rpc exited before the signal; stderr: {}",
        drain_stderr(&mut child)
    );

    kill_with(&child, signal);
    let code = wait_for_exit(&mut child, Duration::from_secs(15));
    if code.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("still alive 15s after the FIRST {signal} — SEAM-047");
    }
    assert_eq!(
        code,
        Some(expected_code),
        "the FIRST {signal} must exit {expected_code} (pi rpc-mode.ts:374); stderr: {}",
        drain_stderr(&mut child)
    );
    drop(stdin);
}

/// pi `rpc-mode.ts:374` — `signal === "SIGHUP" ? 129 : 143`, on the FIRST delivery.
#[test]
fn first_sigterm_exits_143() {
    first_delivery_exits("-TERM", 143);
}

#[test]
fn first_sighup_exits_129() {
    first_delivery_exits("-HUP", 129);
}

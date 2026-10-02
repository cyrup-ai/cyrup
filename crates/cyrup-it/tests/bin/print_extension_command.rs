//! SEAM-137 — `cyrup -p "/<extension command>"` must exit, at the BINARY seam.
//!
//! A prompt an extension command services starts no run, so `session.prompt()`'s run-scoped stream
//! is never closed and no `agent_settled` arrives. Print mode drained that stream, so
//! `cyrup -p "/llama"` and `cyrup -p "/mcp"` ran until killed (observed past 20 s and past 60 s).
//! Pi's print mode awaits `session.prompt(...)`, which resolves as soon as the command's handler
//! has (print-mode.ts:121-127 @v0.99.2-17). The in-process tests live in
//! `crates/cyrup-modes/src/tests/modes/print_mode.rs`; this file proves the shipped executable.
//!
//! `/llama` is the command used because it is registered by default (a hidden built-in), touches
//! nothing outside the process (outside the interactive UI it only warns), and consumes the
//! submission. Fully offline and hermetic, like `one_shot_parity`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Read as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Generous: a healthy run finishes in about a second; the defect never finished at all.
const DEADLINE: Duration = Duration::from_secs(60);

struct Run {
    /// `None` when the deadline passed and the child was killed.
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run the real `cyrup` binary offline with the faux model and the DEFAULT extensions (unlike
/// `one_shot_parity::run`, which passes `--no-extensions`: here the command must be registered),
/// killing it when [`DEADLINE`] passes so a regression fails this test instead of hanging the suite.
fn run(args: &[&str]) -> (Run, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), tmp.path());
    let mut child = cmd
        .current_dir(&work)
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .args(["--offline", "--no-session", "--model", "faux/faux-1"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cyrup");

    // Drained on their own threads so a chatty child cannot fill a pipe and stall.
    let drain = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));

    let started = Instant::now();
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status.code().unwrap_or(-1));
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    (
        Run {
            code,
            stdout: stdout.join().unwrap(),
            stderr: stderr.join().unwrap(),
        },
        tmp,
    )
}

/// THE headline: a one-shot run whose only prompt an extension command handles EXITS, with pi's
/// exit code 0 and nothing on stdout (a handled command produces no assistant message, so the
/// terminal block has nothing to print: print-mode.ts:132).
#[test]
fn a_print_run_whose_only_prompt_is_an_extension_command_exits_zero() {
    let (r, _tmp) = run(&["-p", "/llama"]);
    assert_eq!(
        r.code,
        Some(0),
        "print mode did not exit within {DEADLINE:?} (None = killed) or exited non-zero;\nstdout: {}\nstderr: {}",
        r.stdout,
        r.stderr
    );
    assert!(
        r.stdout.is_empty(),
        "a handled command prints nothing, stdout was: {}",
        r.stdout
    );
}

/// The same prompt under `--mode json`, which already guarded this path: it proves the command was
/// CONSUMED by the extension (no run, so no `agent_start`) rather than sent to the model, which is
/// what makes the print test above a test of the handled-prompt path and not of a fast model turn.
#[test]
fn the_extension_command_consumes_the_prompt_so_no_run_starts() {
    let (r, _tmp) = run(&["--mode", "json", "-p", "/llama"]);
    assert_eq!(
        r.code,
        Some(0),
        "json mode did not exit cleanly;\nstdout: {}\nstderr: {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !r.stdout.contains("\"type\":\"agent_start\""),
        "no run may start for a handled command, stdout was: {}",
        r.stdout
    );
}

/// A command followed by an ordinary prompt: the command is handled and the loop CONTINUES, so the
/// second prompt still starts a run.
///
/// The binary's offline faux model has no scripted reply, so that run FAILS ("No more faux
/// responses queued") and print mode reports it the way pi does for any failed turn: the reason on
/// stderr and exit code 1 (print-mode.ts:133-137, :147). The failure is the evidence: only a prompt
/// that reached the model can produce it. The successful case, with a scripted reply that is
/// printed, is `the_send_loop_continues_after_a_handled_prompt` in `cyrup-modes`.
#[test]
fn a_prompt_after_an_extension_command_still_reaches_the_model() {
    let (r, _tmp) = run(&["-p", "/llama", "say something"]);
    assert_eq!(
        r.code,
        Some(1),
        "print mode did not finish within {DEADLINE:?} (None = killed), or the second prompt never \
         ran (exit 0);\nstdout: {}\nstderr: {}",
        r.stdout,
        r.stderr
    );
    assert!(
        !r.stderr.trim().is_empty(),
        "a failed turn reports its reason on stderr (print-mode.ts:146)"
    );
}

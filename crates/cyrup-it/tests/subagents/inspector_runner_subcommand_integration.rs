//! VL-S6, T-RUN-1/T-RUN-2 — the `__subagent-inspector` seam, end to end, through the REAL binary.
//!
//! # Why this file has to exist, and why nothing else can replace it
//!
//! The inspector pane is a SECOND OS PROCESS that a terminal host — not cyrup — starts, from the
//! string `inspector.command` returns. Reaching it takes two edits in two different crates:
//!
//! * `crates/cyrup/src/predispatch.rs` CLASSIFIES `argv[1] == "__subagent-inspector"`, and
//! * `crates/cyrup/src/main.rs` DISPATCHES that classification into
//!   `cyrup::subagent_inspector_cmd::dispatch`.
//!
//! Classification and dispatch are deliberately split (the module doc at `predispatch.rs:13-19`
//! states why: `set_process_name` needs `unsafe`, which only the binary crate may hold). The
//! consequence is that a HALF-WIRED seam **fails silently**: with the `predispatch` arm present
//! and the `main.rs` arm missing, `classify_internal` answers `Some(SubagentInspector)`, `main`
//! has no branch for it, and the argv falls straight through to clap — which rejects
//! `--async-dir` with a usage error and exit 2. Every unit test in
//! `cyrup_ext_subagents::inspectors::runner` still passes, because the library half is perfect;
//! it is the SEAM that is broken. Only spawning the real binary sees it.
//!
//! So this file asserts the thing that actually matters to a user: `cyrup __subagent-inspector
//! …` against a real on-disk async run renders a real dashboard, and a `steer` line typed into
//! the pane lands a REAL steering request on that run's control channel — the file, not a
//! receipt.
//!
//! Hermetic by construction (`support::env::hermetic`): no ambient credential, `CYRUP_HOME`
//! redirect or opt-in gate can reach the child, and the child needs none — predispatch runs
//! before any config resolution (`main.rs`'s `classify_internal` call sits above the bootstrap
//! proxy install and above `run_predispatch`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use cyrup_ext_subagents::background::{
    RunDir, RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus,
};
use cyrup_ext_subagents::inspectors::session_roots_codec::encode_session_roots;
use cyrup_ext_subagents::inspectors::types::{INSPECTOR_HEADER_PREFIX, INSPECTOR_SUBCOMMAND};
use tempfile::TempDir;

/// How long a filesystem assertion waits for the pane process to act on a line it was fed.
const DEADLINE: Duration = Duration::from_secs(20);

/// A real async run on disk: `<tmp>/async/<run_id>/status.json`, `Running`, one running child.
///
/// Written with plain `serde_json` rather than `write_atomic_json` on purpose — this test is
/// asserting what the PANE does, so the seeding half must not depend on the same crate's async
/// machinery to be correct.
fn seed_running_run(tmp: &Path, run_id: &str) -> PathBuf {
    let async_root = tmp.join("async");
    let run = RunId::from_token(run_id.to_string());
    let paths = RunPaths::for_run(&async_root, &tmp.join("results"), &run);
    std::fs::create_dir_all(&paths.run_dir).unwrap();

    let mut running = StepStatus::pending("worker");
    running.status = StepState::Running;
    let mut status = RunStatus::queued(run, RunMode::Single, Some(std::process::id()));
    status.state = RunState::Running;
    status.steps = vec![running];
    std::fs::write(&paths.status, serde_json::to_vec(&status).unwrap()).unwrap();
    paths.run_dir
}

/// Flip the seeded run to `Complete`, which is what makes the pane's refresh timer stop.
fn settle_run(run_dir: &Path, run_id: &str) {
    let path = RunDir::for_existing(run_dir).status();
    let bytes = std::fs::read(&path).unwrap();
    let mut status: RunStatus = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status.run_id.as_str(), run_id);
    status.state = RunState::Complete;
    status.steps[0].status = StepState::Complete;
    std::fs::write(&path, serde_json::to_vec(&status).unwrap()).unwrap();
}

/// Every JSON file directly under `dir`, parsed — `[]` while the directory does not exist yet.
fn control_requests(dir: &Path) -> Vec<serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if let Ok(bytes) = std::fs::read(entry.path())
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            out.push(value);
        }
    }
    out
}

/// Poll `dir` until one request satisfying `predicate` appears, or fail with what was there.
fn await_control_request(
    dir: &Path,
    label: &str,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let found = control_requests(dir);
        if let Some(hit) = found.iter().find(|value| predicate(value)) {
            return hit.clone();
        }
        assert!(
            Instant::now() < deadline,
            "no {label} request matching the predicate landed in {} within {DEADLINE:?}; saw \
             {found:#?}",
            dir.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// **T-RUN-1.** The whole seam: classify, dispatch, render, and two control verbs that write real
/// files on the run's control channel.
///
/// GUT (classification): delete the `subagent_inspector_cmd::is_selected` arm from
/// `predispatch::classify_internal` — the argv reaches clap, the child exits non-zero with a
/// usage error, and the FIRST assertion (`stdout contains the header`) goes red.
///
/// GUT (dispatch): delete the `Some(Internal::SubagentInspector)` arm from `main.rs` — the
/// classification still answers correctly and every `inspectors::runner` unit test still passes,
/// but the argv falls through to clap exactly as above and this goes red. **This is the half
/// nothing else in the suite can see.**
///
/// GUT (the control channel): delete the `request_async_steer` call in
/// `inspectors::runner::queue_inspector_steer` — the pane still prints "Steering queued", and the
/// steer-request assertion below goes red because it reads the FILE.
#[test]
fn cyrup_subagent_inspector_renders_a_real_run_and_a_steer_line_lands() {
    let tmp = TempDir::new().unwrap();
    let run_id = "inspector-run-1";
    let run_dir = seed_running_run(tmp.path(), run_id);
    let session_root = tmp.path().join("work");
    std::fs::create_dir_all(&session_root).unwrap();
    let roots = encode_session_roots(&[session_root.clone()]);

    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), tmp.path());
    cmd.arg(INSPECTOR_SUBCOMMAND)
        .arg("--async-dir")
        .arg(&run_dir)
        .arg("--run-id")
        .arg(run_id)
        .arg("--allow-steer")
        .arg("true")
        .arg("--allow-stop")
        .arg("true")
        .arg("--session-roots")
        .arg(&roots)
        .arg("--refresh-ms")
        .arg("250")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("the real cyrup binary spawns");
    let mut stdin = child.stdin.take().expect("stdin is piped");

    // Drain both pipes on their own threads FOR THE WHOLE RUN, not after `wait`. A pane repaints
    // every `--refresh-ms`, so a test that only reads at the end deadlocks the moment the 64 KiB
    // pipe buffer fills: the child blocks inside `write_all`, never reaches the next control line,
    // and the filesystem poll below times out for a reason that has nothing to do with the seam.
    let mut child_stdout = child.stdout.take().expect("stdout is piped");
    let mut child_stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = std::io::Read::read_to_end(&mut child_stdout, &mut buffer);
        buffer
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = std::io::Read::read_to_end(&mut child_stderr, &mut buffer);
        buffer
    });

    // One control line at a time, each asserted on the FILESYSTEM before the next is written, so
    // a failure names which verb did not land rather than "nothing happened".
    stdin.write_all(b"steer hello from the pane\n").unwrap();
    stdin.flush().unwrap();
    let steer = await_control_request(
        &cyrup_ext_subagents::background::control::steer_requests_dir(&run_dir),
        "steer",
        |value| value.get("source").and_then(serde_json::Value::as_str) == Some("inspector-runner"),
    );
    assert_eq!(
        steer.get("message").and_then(serde_json::Value::as_str),
        Some("hello from the pane"),
        "the pane must forward the message verbatim: {steer:#?}"
    );
    assert_eq!(
        steer.get("targetIndex").and_then(serde_json::Value::as_u64),
        Some(0),
        "a `single`-mode run addresses child 0 (`inspector-runner.ts:89`): {steer:#?}"
    );

    stdin.write_all(b"stop\n").unwrap();
    stdin.flush().unwrap();
    let stop = await_control_request(
        &cyrup_ext_subagents::background::control::stop_requests_dir(&run_dir),
        "stop",
        |value| value.get("source").and_then(serde_json::Value::as_str) == Some("inspector-runner"),
    );
    assert_eq!(
        stop.get("type").and_then(serde_json::Value::as_str),
        Some("stop"),
        "{stop:#?}"
    );

    // Terminalise the run under the live pane, then close stdin. A pane whose refresh timer kept
    // running would still exit here (EOF ends the loop either way) — what this proves is that
    // reaching a terminal state does not make it hang, crash or exit non-zero.
    settle_run(&run_dir, run_id);
    drop(stdin);

    let status = child.wait().expect("the pane process exits");
    let stdout = String::from_utf8_lossy(&stdout_reader.join().expect("stdout drain")).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().expect("stderr drain")).into_owned();
    assert!(
        status.success(),
        "the pane must exit 0; a usage error here means the seam is half-wired (predispatch \
         without the main.rs arm). code={:?} stderr={stderr}",
        status.code()
    );
    assert!(
        stdout.contains(&format!("{INSPECTOR_HEADER_PREFIX}{run_id}")),
        "the dashboard header is missing — the argv never reached the runner. stdout={stdout} \
         stderr={stderr}"
    );
    assert!(
        stdout.contains(
            "This inspector mirrors lifecycle artifacts; closing it does not stop the run."
        ),
        "stdout={stdout}"
    );
    assert!(
        stdout.contains("Controls: type guidance | steer <message> | stop | status"),
        "both authority flags were `true`, so every control must be offered: stdout={stdout}"
    );
    assert!(
        stdout.contains("Steering queued for run inspector-run-1."),
        "the steer receipt must have been rendered back into the pane: stdout={stdout}"
    );
    assert!(
        stdout.contains("Stop requested for run inspector-run-1."),
        "stdout={stdout}"
    );
    assert!(
        stdout.contains(&format!("Run: {run_id}")),
        "the body must be the real `format_async_run_transcript` output: stdout={stdout}"
    );
}

/// The refusals the pane prints are the runner's own, not clap's — a second, cheaper witness that
/// the argv is being parsed by [`cyrup_ext_subagents::inspectors::runner::parse_args`] and not by
/// the user-facing CLI.
///
/// GUT: delete either seam arm — clap answers instead, with its own usage text, and the
/// `Inspector failed:` assertion goes red.
#[test]
fn a_bad_inspector_argv_is_refused_by_the_runners_own_parser() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), tmp.path());
    let output = cmd
        .arg(INSPECTOR_SUBCOMMAND)
        .arg("--async-dir")
        .arg(tmp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the real cyrup binary spawns");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(1),
        "upstream exits 1 on a refused argv (`inspector-runner.ts:151`); stderr={stderr}"
    );
    assert!(
        stderr.contains("Inspector failed: Inspector requires --async-dir and --run-id."),
        "the message must be the runner's own sentence, not a clap usage error: stderr={stderr}"
    );
}

/// **T-RUN-2.** The token stays undiscoverable: `cyrup --help` advertises neither internal
/// subcommand, and neither resolves through the user-facing subcommand surface.
///
/// This is the other half of the `[CYRUP-DELTA] (SEAM-109)` premise — the delta is only honest
/// while the token is absent from every surface a user reads.
///
/// GUT: add `"__subagent-inspector"` to `crate::subcommands::SUBCOMMANDS`, or advertise it in the
/// help text — this goes red.
#[test]
fn the_inspector_subcommand_is_undiscoverable() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), tmp.path());
    let output = cmd
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the real cyrup binary spawns");
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !help.contains(INSPECTOR_SUBCOMMAND),
        "`--help` must not advertise the inspector hop: {help}"
    );
    assert!(
        !help.contains("__subagent-runner"),
        "nor its sibling: {help}"
    );
    assert!(
        help.len() > 200,
        "the help text itself must have been produced — an empty answer would make the two \
         assertions above vacuous: {help}"
    );
}

//! SEAM-106 — `--export` runs where pi runs it: immediately after the `--version` exit, UPSTREAM of
//! every guard that can refuse a run.
//!
//! pi v0.83.0 `packages/coding-agent/src/main.ts:578-590` puts the export branch between the
//! `--version` exit (`:573-576`) and `resolveAppMode` (`:592`). Four guards that used to precede
//! cyrup's copy therefore run AFTER upstream's:
//!
//! * `validateForkFlags` / `validateSessionIdFlags` (`:603-604`),
//! * the RPC `@file` guard (`:598-601`),
//! * the `--api-key requires a model` bail (`:757-761`).
//!
//! Export is the operation a user reaches for when the session is ALREADY in a bad state, so the
//! guards fired on exactly the invocations that needed it most: `cyrup --export s.jsonl --api-key K`
//! and `cyrup --export s.jsonl --fork X --continue` both errored where pi writes the HTML and exits
//! 0.
//!
//! The optional output path is pi's `parsed.messages[0]` (`:580`) — the MESSAGE list, from which
//! `@file` tokens were already partitioned away (`cli/args.ts:186-187`). cyrup read
//! `cli.positionals.first()`, which still holds them, so `cyrup --export s.jsonl @notes.md` wrote
//! its HTML to a file literally named `@notes.md`.
//!
//! This drives the REAL binary — dispatch ORDER is not observable from a unit test. Every case is
//! offline and hermetic (tempdir agent dir + cwd, `--offline`); nothing here can reach a provider.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// A minimal session transcript. `session_jsonl_to_html` tolerates whatever it does not recognise,
/// so the one thing this needs to be is a file that exists and parses as JSONL.
const SESSION_JSONL: &str = concat!(
    r#"{"type":"session-header","version":1,"id":"11111111-1111-4111-8111-111111111111"}"#,
    "\n",
    r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
    "\n",
);

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run the real binary in a hermetic tempdir. `args` are appended after the session path.
fn export_run(tmp: &Path, args: &[&str]) -> Run {
    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.current_dir(&work)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", tmp)
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .env("CYRUP_OFFLINE", "1")
        .args(args);
    let out = cmd.output().expect("spawn cyrup");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Write the session file and return its path.
fn session_file(tmp: &Path) -> std::path::PathBuf {
    let path = tmp.join("session.jsonl");
    std::fs::write(&path, SESSION_JSONL).unwrap();
    path
}

/// Presence before absence: the plain export still works, and still writes `<input>.html`.
#[test]
fn plain_export_writes_the_sibling_html_and_exits_zero() {
    let tmp = TempDir::new().unwrap();
    let session = session_file(tmp.path());
    let run = export_run(tmp.path(), &["--export", session.to_str().unwrap()]);
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(run.stdout.contains("Exported to:"), "{}", run.stdout);
    assert!(session.with_extension("html").exists());
}

/// `--api-key K` with no model is a hard bail at pi `main.ts:757-761` — but that is 170 lines BELOW
/// the export branch, so upstream never reaches it on an export run.
#[test]
fn export_runs_upstream_of_the_api_key_requires_a_model_bail() {
    let tmp = TempDir::new().unwrap();
    let session = session_file(tmp.path());
    let run = export_run(
        tmp.path(),
        &[
            "--export",
            session.to_str().unwrap(),
            "--api-key",
            "sk-not-used",
        ],
    );
    assert_eq!(
        run.code, 0,
        "the export must not be refused by a guard pi runs after it; stderr: {}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("--api-key requires a model"),
        "{}",
        run.stderr
    );
    assert!(session.with_extension("html").exists());
}

/// `--fork X --continue` is `validateForkFlags` (pi `main.ts:603`), likewise below the export branch.
#[test]
fn export_runs_upstream_of_the_conflicting_session_flag_guards() {
    let tmp = TempDir::new().unwrap();
    let session = session_file(tmp.path());
    let run = export_run(
        tmp.path(),
        &[
            "--export",
            session.to_str().unwrap(),
            "--fork",
            "deadbeef",
            "--continue",
        ],
    );
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(session.with_extension("html").exists());
}

/// The output path comes from pi's `messages`, not from the raw positionals: an `@file` token is a
/// FILE ARG upstream and can never become the export destination.
#[test]
fn the_output_path_comes_from_messages_not_from_file_args() {
    let tmp = TempDir::new().unwrap();
    let session = session_file(tmp.path());
    let work = tmp.path().join("work");
    let run = export_run(
        tmp.path(),
        &["--export", session.to_str().unwrap(), "@notes.md"],
    );
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(
        !work.join("@notes.md").exists(),
        "an `@file` token must never be taken as the export destination"
    );
    assert!(session.with_extension("html").exists());

    // Presence before absence: a real message positional IS the destination (pi `messages[0]`).
    let named = export_run(
        tmp.path(),
        &["--export", session.to_str().unwrap(), "out.html"],
    );
    assert_eq!(named.code, 0, "stderr: {}", named.stderr);
    assert!(work.join("out.html").exists(), "{}", named.stdout);
}

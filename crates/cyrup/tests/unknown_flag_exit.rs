//! SEAM-S01 end-to-end — an unknown `--flag` must be an ERROR with exit 1, not silence.
//!
//! Pi's hand-rolled parser captures every unrecognised `--flag` into `unknownFlags`
//! (`cli/args.ts:205-215`) and defers judgement until the extensions have loaded and declared their
//! own flags. `applyExtensionFlagValues` then reconciles: any captured name no loaded extension
//! registered becomes `{type:"error", message:"Unknown option(s): --foo"}`
//! (`core/agent-session-services.ts:98-125`), which merges into `services.diagnostics` (`:182`) →
//! `runtime.diagnostics` → `reportDiagnostics(...)` + `process.exit(1)` (`main.ts:843-848`).
//!
//! cyrup had BOTH ends of that machinery and never connected them: the capture
//! (`cli.rs::partition_extension_flags`) and the reporter (`main.rs::report_diagnostics` + `Ok(1)`),
//! but `apply_extension_flag_values` `continue`d past the unknown name and
//! `AgentSessionRuntime::diagnostics()` had no production consumer at all. A typo was a silent
//! no-op with exit 0.
//!
//! This drives the REAL binary — the only way to observe the exit code and the stderr text
//! together. Fully offline (`--offline`, faux model, tempdir HOME/agent dir); no network, no
//! provider credentials.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::process::{Command, Stdio};

use tempfile::TempDir;

struct Run {
    code: i32,
    stderr: String,
    stdout: String,
}

/// Run the real `cyrup` binary in a hermetic tempdir with the offline faux model.
fn run(extra: &[&str]) -> (Run, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.current_dir(&work)
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent_dir)
        // Never inherit an ambient key or proxy — this test must not be able to reach a network.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .args(["--offline", "--no-session", "--no-extensions", "--model", "faux/faux-1"])
        .args(extra)
        .args(["-p", "hi"])
        .stdin(Stdio::null());
    let out = cmd.output().expect("spawn cyrup");
    (
        Run {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        },
        tmp,
    )
}

/// THE headline: a mistyped flag names itself on stderr and exits 1.
#[test]
fn a_mistyped_long_flag_reports_unknown_option_and_exits_1() {
    let (r, _tmp) = run(&["--dangerously-skip-permissions"]);
    assert_eq!(r.code, 1, "stderr was: {}", r.stderr);
    assert!(
        r.stderr.contains("Error: Unknown option: --dangerously-skip-permissions"),
        "stderr was: {}",
        r.stderr
    );
    // The error goes to stderr; stdout stays clean for the protocol stream.
    assert!(!r.stdout.contains("Unknown option"), "stdout was: {}", r.stdout);
}

/// A `--flag value` pair is captured with its value and still reported by NAME; two unknowns
/// collapse into Pi's single pluralized message.
#[test]
fn two_unknown_flags_report_once_in_the_plural() {
    let (r, _tmp) = run(&["--persona", "reviewer", "--depth", "3"]);
    assert_eq!(r.code, 1, "stderr was: {}", r.stderr);
    assert!(
        r.stderr.contains("Error: Unknown options: --persona, --depth"),
        "stderr was: {}",
        r.stderr
    );
}

/// The CONTROL that keeps this from being "everything exits 1": with no unknown flag the run gets
/// PAST the diagnostics checkpoint and into dispatch — proven by the faux provider's own
/// out-of-responses message, which can only be reached after the session is built and prompted.
#[test]
fn a_clean_invocation_passes_the_checkpoint_untouched() {
    let (r, _tmp) = run(&[]);
    assert!(
        !r.stderr.contains("Unknown option"),
        "a clean argv must produce no unknown-option diagnostic: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("No more faux responses queued"),
        "the run must reach dispatch, i.e. past the checkpoint; stderr: {}",
        r.stderr
    );
}

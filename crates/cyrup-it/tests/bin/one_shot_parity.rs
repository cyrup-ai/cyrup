//! Two one-shot CLI parity defects, proved at the BINARY seam — the only place the exit code and
//! the stderr text are observable together.
//!
//! 1. **Scope diagnostics were silently dropped.** Pi resolves `--models` / `enabledModels` through
//!    `resolveModelScope` (model-resolver.ts:355-361), which prints every diagnostic collected by
//!    `resolveModelScopeWithDiagnostics` — `console.warn(chalk.yellow(`Warning: ${d.message}`))` —
//!    including `No models match pattern "<p>"` for an unmatched glob (:311-318) or non-glob
//!    (:334-341) pattern. That is the live path (`main.ts:741-743`, over
//!    `parsed.models ?? settingsManager.getEnabledModels()`). cyrup called
//!    `ModelResolver::resolve_scope`, which returns ONLY the matched set, and then did
//!    `if !scoped.is_empty()` — so a typo'd `--models "anthropc/*"` scoped nothing, printed nothing
//!    and launched unscoped.
//!
//! 2. **A prompt-less one-shot run was a hard error.** Pi's `buildInitialMessage` answers
//!    `initialMessage: undefined` when there is no stdin, no `@file` and no message
//!    (initial-message.ts:36-42); `runPrintMode` then skips both send loops (print-mode.ts:121-127)
//!    and falls through to the terminal output block, returning the `exitCode = 0` from :34. JSON
//!    mode has already written the session header by then (:112-118). cyrup's `main.rs::ensure_prompt`
//!    bailed instead — `cyrup: no prompt provided: …` and exit 1 — inverting the exit code of
//!    `cyrup -c -p` and emitting no JSON header at all.
//!
//! Fully offline: `--offline`, the faux model, a tempdir HOME/agent dir, every provider key and
//! proxy scrubbed from the child env. No network, no credentials, no paid tokens.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::{Command, Stdio};

use tempfile::TempDir;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run the real `cyrup` binary in a hermetic tempdir with the offline faux model. `args` is appended
/// verbatim, so a test controls both the flags and whether a prompt is present.
fn run(args: &[&str]) -> (Run, TempDir) {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = Command::new(crate::support::bins::cyrup());
    cmd.current_dir(&work)
        .env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent_dir)
        // Never inherit an ambient key or proxy — this test must not be able to reach a network.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        // ...and never inherit an ambient BUILT-IN OPT-IN either. `CYRUP_INTERCOM=1` alone
        // satisfies `is_installed()` (`cyrup-intercom/src/extension.rs:630-631`, env var name at
        // `:87`) even though this tempdir agent dir holds no `intercom/config.json`, so the child
        // attaches intercom and detaches a real `__intercom-broker`. That broker never self-exits —
        // `schedule_shutdown_check` is armed only by a REGISTERED session's disconnect (1:1 with
        // pi-intercom `broker/broker.ts:221`/`:429`), and a one-shot run exits before its connect
        // task registers — so it outlives cargo. Measured on a developer box that exports all three
        // vars: the four binary-seam targets in this crate left 13 immortal brokers per run, 0
        // under `env -u CYRUP_INTERCOM`. A hermetic run's extension set must come from the fixture,
        // not the developer's shell; `auth_credential_print.rs` takes the stronger `env_clear` +
        // allowlist form of the same rule.
        .env_remove("CYRUP_INTERCOM")
        .env_remove("CYRUP_SUBAGENTS")
        .env_remove("CYRUP_PERMISSION_SYSTEM")
        .args([
            "--offline",
            "--no-session",
            "--no-extensions",
            "--model",
            "faux/faux-1",
        ])
        .args(args)
        // A null stdin is a non-TTY at EOF: `read_piped_stdin` yields no piped text, exactly like
        // pi's `stdinContent === undefined`.
        .stdin(Stdio::null());
    let out = cmd.output().expect("spawn cyrup");
    (
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        tmp,
    )
}

/// THE headline for defect 1: a `--models` pattern that matches nothing names itself on stderr
/// (Pi `Warning: No models match pattern "…"`, model-resolver.ts:314 + :359).
#[test]
fn an_unmatched_models_pattern_warns_on_stderr() {
    let (r, _tmp) = run(&["--models", "anthropc/*", "-p", "hi"]);
    assert!(
        r.stderr
            .contains("Warning: No models match pattern \"anthropc/*\""),
        "stderr was: {}\nstdout was: {}",
        r.stderr,
        r.stdout
    );
    // The diagnostic goes to stderr only — stdout stays clean for the one-shot payload.
    assert!(
        !r.stdout.contains("No models match pattern"),
        "stdout was: {}",
        r.stdout
    );
}

/// A `--models` pattern that DOES match stays silent — the warning must not fire on the happy path.
#[test]
fn a_matching_models_pattern_emits_no_warning() {
    let (r, _tmp) = run(&["--models", "faux/*", "-p", "hi"]);
    assert!(
        !r.stderr.contains("No models match pattern"),
        "a matching pattern must be silent, stderr was: {}",
        r.stderr
    );
}

/// THE headline for defect 2: a one-shot run with no prompt at all exits **0** and never prints
/// cyrup's invented `no prompt provided` error (Pi `runPrintMode` skips its send loops and returns
/// `exitCode = 0`, print-mode.ts:34,121-146).
#[test]
fn a_prompt_less_print_run_exits_zero_without_an_error() {
    let (r, _tmp) = run(&["-p"]);
    assert!(
        !r.stderr.contains("no prompt provided"),
        "pi has no prompt-required guard, stderr was: {}",
        r.stderr
    );
    assert_eq!(
        r.code, 0,
        "expected pi's exit 0; stderr was: {}\nstdout was: {}",
        r.stderr, r.stdout
    );
    // A fresh prompt-less session has no assistant message, so the terminal output block writes
    // nothing (Pi `if (lastMessage?.role === "assistant")`, print-mode.ts:132).
    assert!(r.stdout.is_empty(), "stdout was: {}", r.stdout);
}

/// The JSON-mode half of defect 2: pi writes the session header BEFORE `rebindSession()`
/// (print-mode.ts:112-118), so a prompt-less `--mode json` run still emits its one header line and
/// exits 0. cyrup's guard errored before anything reached stdout.
#[test]
fn a_prompt_less_json_run_still_emits_the_session_header() {
    let (r, _tmp) = run(&["--mode", "json"]);
    assert_eq!(
        r.code, 0,
        "expected pi's exit 0; stderr was: {}\nstdout was: {}",
        r.stderr, r.stdout
    );
    let first = r.stdout.lines().next().unwrap_or_default();
    let header: serde_json::Value =
        serde_json::from_str(first).unwrap_or_else(|e| panic!("header line {first:?}: {e}"));
    assert_eq!(
        header.get("type").and_then(|t| t.as_str()),
        Some("session"),
        "the first JSONL line is the session header, got: {first}"
    );
}

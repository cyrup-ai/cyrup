//! `cyrup-subagent-fixture` — the scripted-NDJSON test-double `cyrup`-shaped binary (arch-SA
//! §11), gated behind the `test-fixtures` Cargo feature and never built for/shipped inside the
//! real `cyrup` binary (see `Cargo.toml`'s `[[bin]]` entry for the exact gating).
//!
//! # Purpose
//!
//! This crate's own mandated mechanism (func-SA §1.1) is that a subagent run is ALWAYS a genuine
//! OS subprocess re-exec of the real `cyrup` binary. Exercising that mechanism in tests therefore
//! requires a REAL child process to spawn — never a mock — but actually re-execing the full
//! `cyrup` binary (with real providers, real model calls) from a unit/integration test is neither
//! deterministic nor fast. This binary is the standard, deliberate substitute: a tiny, real OS
//! process that speaks the exact same wire protocol (`--print --mode json` NDJSON-on-stdout) a
//! real subagent child does, driven entirely by a small JSON script file so a test can assert
//! exact, repeatable behavior (specific events, specific timing, specific signal responses)
//! without any network/model dependency. Tests substitute it for the real binary via
//! `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1) — [`crate::spawn::resolve_spawn_command`]'s own
//! documented override escape hatch — exactly the mechanism a real scripted-binary substitution
//! is meant to use in production tooling.
//!
//! # Script format
//!
//! The fixture reads a script from the path named by the `CYRUP_SUBAGENT_FIXTURE_SCRIPT`
//! environment variable (a required argument for any test that wants specific, non-default
//! behavior; see [`FixtureScript`]'s `Default` impl for the no-script fallback behavior a test
//! that only cares about argv/env echoing can rely on instead of writing a script file at all).
//!
//! ```json
//! {
//!   "steps": [
//!     { "kind": "emit", "line": "{\"type\":\"agent_start\"}" },
//!     { "kind": "sleep_ms", "ms": 200 },
//!     { "kind": "emit", "line": "{\"type\":\"agent_end\"}" }
//!   ],
//!   "echo_argv": true,
//!   "echo_env": ["CYRUP_SUBAGENT_DEPTH", "CYRUP_SUBAGENT_MAX_DEPTH"],
//!   "ignore_sigint": false,
//!   "ignore_sigterm": false,
//!   "exit_code": 0
//! }
//! ```
//!
//! - `steps`: an ordered list of [`ScriptStep`]s — `emit` writes one raw line (already
//!   newline-terminated by this binary, so `line` itself should NOT include a trailing `\n`) to
//!   stdout and flushes immediately (mirroring `spawn::SpawnedChild`'s own "live, not
//!   buffered-to-exit" contract, so a test reading the fixture's stdout observes each line as
//!   soon as it is written); `sleep_ms` sleeps for the given duration before continuing, letting
//!   tests script a deliberately slow/straggling child (R-SA-068's final-drain scenario) or a
//!   child that is still "mid-run" when a timeout/cancel/interrupt fires.
//! - `echo_argv`: when `true`, emits one `{"type":"unknown","arg":"<value>"}` line per argv
//!   entry received (after the fixture's own binary-name arg[0]), for argv-ordering assertions
//!   (mirrors `spawn/mod.rs`'s own existing `sh`-based argv-echo tests, ported to this fixture so
//!   later phases can reuse the exact same assertion style without hand-rolling a `sh -c` script).
//! - `echo_env`: a list of env-var names; for each one present, emits one
//!   `{"type":"unknown","env":"<NAME>","value":"<value>"}` line — this is exactly what
//!   architecture.md §11's depth-tightening test ("assert its own emitted env... reflects
//!   `min(inherited, agent)`") needs: a test sets `CYRUP_SUBAGENT_MAX_DEPTH` in the child's
//!   overlay env, and this fixture echoes it back as an NDJSON event rather than the test needing
//!   any other channel to observe what the child actually saw.
//! - `ignore_sigint`/`ignore_sigterm`: when `true`, the fixture installs a signal handler that
//!   swallows that signal instead of the OS default terminate-on-signal behavior — this is what
//!   lets a test deterministically exercise [`crate::spawn::signal::terminate`]'s full
//!   SIGINT->SIGTERM->SIGKILL escalation ladder (a fixture that ignores both SIGINT and SIGTERM
//!   can ONLY be killed by SIGKILL, which cannot be caught/ignored/blocked by any conforming OS —
//!   exactly the scripted-child-that-ignores-SIGINT scenario architecture.md §11's kill-escalation
//!   test (A-SA-11) names explicitly).
//! - `exit_code`: the process exit code once every step has run and stdout has been flushed
//!   (default `0`).
//!
//! A missing/unreadable script file, or a script file that fails to parse as valid JSON, degrades
//! to [`FixtureScript::default`] (emit nothing beyond argv/env echoing, if requested via env vars
//! — see below — exit 0 immediately) rather than this binary panicking or erroring loudly: a test
//! harness bug that fails to write its intended script file should not make failures here any
//! harder to diagnose than "the fixture behaved like the default", which is itself immediately
//! visible in test output.
//!
//! As a convenience for tests that want argv/env echoing WITHOUT writing a script file at all,
//! `CYRUP_SUBAGENT_FIXTURE_ECHO_ARGV=1` and `CYRUP_SUBAGENT_FIXTURE_ECHO_ENV=NAME1,NAME2` are
//! read as a fallback when `CYRUP_SUBAGENT_FIXTURE_SCRIPT` is absent — this mirrors this binary's
//! own "driven entirely by env vars/argv, no interactive input" design (it never reads stdin,
//! matching R-SA-046's `stdin: null` contract every real subagent child is spawned under).
//!
//! This binary has ZERO dependency on any provider/model/session machinery — it is a pure,
//! self-contained NDJSON emitter, deliberately as small as possible so it starts and exits fast
//! across the dozens of real-subprocess tests this crate's testing strategy (arch-SA §11) calls
//! for.

use std::io::Write;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum ScriptStep {
    /// Write `line` (plus a trailing `\n`) to stdout and flush immediately.
    Emit { line: String },
    /// Write `line` (plus a trailing `\n`) to STDERR — for exercising the executor's
    /// stderr-into-error surfacing on a non-zero exit (pi `execution.ts:686`). stderr is not
    /// protocol data (R-SA-046), so this is diagnostic text a real child could equally emit.
    EmitStderr { line: String },
    /// Write ONE line of `head` + `pad_bytes` `'x'` bytes + `tail` (plus a trailing `\n`) — the
    /// only way to script a child whose single stdout line exceeds the parent's per-line cap
    /// (`crate::exec::child_protocol::MAX_CHILD_PENDING_LINE_BYTES`, 16 MiB) without putting 16 MiB
    /// of literal text in the script file. Written in chunks and flushed once, so the parent reads
    /// it exactly as it would a real child's oversized record: many partial reads, no newline.
    EmitPadded {
        head: String,
        pad_bytes: usize,
        tail: String,
    },
    /// [`ScriptStep::EmitPadded`]'s STDERR twin: ONE stderr line of `head` + `pad_bytes` `'x'`
    /// bytes + `tail` (plus a trailing `\n`), written in 64 KiB chunks.
    ///
    /// Two distinct parent-side behaviours need a child that writes MORE stderr than fits in the
    /// OS pipe buffer (~64 KiB on Linux), and neither can be scripted with
    /// [`ScriptStep::EmitStderr`] without embedding hundreds of kilobytes of literal text in the
    /// script JSON:
    ///
    /// 1. **The pipe-buffer deadlock.** A child blocks in `write(2)` once the pipe is full, so a
    ///    parent that does not drain stderr CONCURRENTLY with the run waits forever for an exit
    ///    that can never happen. pi drains on every chunk (`execution.ts:1056-1059`:
    ///    `proc.stderr.on("data", …)`), so its children never block.
    /// 2. **The bounded TAIL.** pi retains the LAST `MAX_CHILD_STDERR_BYTES` of stderr
    ///    (`createBoundedByteTail`, `child-protocol.ts:377-392`, fed raw chunks at
    ///    `execution.ts:1057`), because a fatal error is at the END of a child's stderr. A
    ///    distinctive `tail` here is what lets a test assert the tail survived rather than the head.
    ///
    /// The bytes are `'x'` (not newline-bearing) so the whole thing really is ONE line, exceeding
    /// the parent's per-line stderr bound as well as the pipe buffer.
    EmitStderrPadded {
        head: String,
        pad_bytes: usize,
        tail: String,
    },
    /// Sleep for `ms` milliseconds before continuing to the next step.
    SleepMs { ms: u64 },
    /// Play the role of a real child's `structured_output` TOOL CALL: write `value` verbatim, as
    /// JSON, to the capture path the parent handed this process in
    /// `CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE` (pi `structured-output.ts:9`'s
    /// `PI_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE`; the child-side writer is
    /// `subagent-prompt-runtime.ts`'s registered `structured_output` tool).
    ///
    /// This is the ONLY channel `exec::structured::read_structured_output` consults when the
    /// parent created a capture runtime — a fenced ` ```json ` block in prose is deliberately NOT
    /// a substitute (pi's whole point at `structured-output.ts:157-159`: a missing capture file is a
    /// hard failure even when prose was produced). A fixture test that wants to exercise the
    /// structured-output path therefore has to make this call, exactly as a real child would.
    ///
    /// With the env var absent (no schema was declared for this run) the step is a silent no-op,
    /// matching a real child, which registers no `structured_output` tool at all in that case.
    /// A write failure is likewise not fatal here: this binary's whole contract (module doc) is
    /// that a scripting mistake degrades to observable-but-not-crashing behaviour, and the parent
    /// then reports its own genuine "missing structured output" failure.
    WriteStructuredOutput { value: serde_json::Value },
}

/// The capture-path env var [`ScriptStep::WriteStructuredOutput`] honours — the same constant
/// `crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV` defines, restated here because this
/// binary deliberately links nothing from the library (see this module's doc: it is a
/// self-contained NDJSON emitter with zero session/provider machinery).
const STRUCTURED_OUTPUT_CAPTURE_ENV: &str = "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE";

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct FixtureScript {
    steps: Vec<ScriptStep>,
    echo_argv: bool,
    echo_env: Vec<String>,
    ignore_sigint: bool,
    ignore_sigterm: bool,
    exit_code: i32,
}

const SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";
const ECHO_ARGV_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_ECHO_ARGV";
const ECHO_ENV_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_ECHO_ENV";

/// Load the fixture's script: `CYRUP_SUBAGENT_FIXTURE_SCRIPT`'s file contents, parsed as
/// [`FixtureScript`] JSON, falling back to [`FixtureScript::default`] (optionally augmented by
/// the `CYRUP_SUBAGENT_FIXTURE_ECHO_ARGV`/`CYRUP_SUBAGENT_FIXTURE_ECHO_ENV` convenience env vars)
/// on any missing-file/read-error/parse-error path — this function never panics and never exits
/// non-zero itself; a malformed script degrades to "do the minimum" rather than aborting the
/// fixture process in a way that would itself look like a signal-escalation-worthy hang to a test
/// harness driving it.
fn load_script() -> FixtureScript {
    let Some(script_path) = std::env::var_os(SCRIPT_ENV_VAR) else {
        return default_script_from_env_fallback();
    };
    let Ok(contents) = std::fs::read_to_string(&script_path) else {
        return default_script_from_env_fallback();
    };
    serde_json::from_str(&contents).unwrap_or_else(|_| default_script_from_env_fallback())
}

/// The env-var-only convenience path (module doc above): no script file at all, but
/// `CYRUP_SUBAGENT_FIXTURE_ECHO_ARGV`/`CYRUP_SUBAGENT_FIXTURE_ECHO_ENV` may still request
/// argv/env echoing.
fn default_script_from_env_fallback() -> FixtureScript {
    let echo_argv = std::env::var(ECHO_ARGV_ENV_VAR)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let echo_env = std::env::var(ECHO_ENV_ENV_VAR)
        .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    FixtureScript {
        echo_argv,
        echo_env,
        ..FixtureScript::default()
    }
}

/// Emit one `{"type":"unknown","arg":"<value>"}` line per argv entry received (after this
/// binary's own `arg[0]`), matching `spawn/mod.rs`'s own existing `sh`-based argv-echo test
/// convention so later phases' tests can assert on the identical event shape without depending on
/// `sh` being present on the test host at all.
fn emit_argv_echo(out: &mut impl Write) {
    for arg in std::env::args().skip(1) {
        let line = serde_json::json!({"type": "unknown", "arg": arg}).to_string();
        let _ = writeln!(out, "{line}");
    }
    let _ = out.flush();
}

/// Emit one `{"type":"unknown","env":"<NAME>","value":"<value>"}` line per requested env-var name
/// that is actually present in this process's environment — absent names are silently skipped
/// (not echoed as `null`/empty), so a test can distinguish "the var was never set at all" from
/// "the var was set to an empty string" by the mere presence/absence of the corresponding line.
fn emit_env_echo(names: &[String], out: &mut impl Write) {
    for name in names {
        if let Ok(value) = std::env::var(name) {
            let line = serde_json::json!({"type": "unknown", "env": name, "value": value}).to_string();
            let _ = writeln!(out, "{line}");
        }
    }
    let _ = out.flush();
}

/// [`ScriptStep::WriteStructuredOutput`]'s worker: write `value` to the parent-provided capture
/// path. Absent env var => no-op (see the variant's own doc for why both degradations are correct).
fn write_structured_output_capture(value: &serde_json::Value) {
    let Some(path) = std::env::var_os(STRUCTURED_OUTPUT_CAPTURE_ENV) else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(value) else {
        return;
    };
    if let Err(err) = std::fs::write(&path, bytes) {
        // stderr is diagnostic, never protocol data (R-SA-046).
        eprintln!("cyrup-subagent-fixture: failed to write structured output capture: {err}");
    }
}

/// Install best-effort signal-ignoring handlers per the script's `ignore_sigint`/`ignore_sigterm`
/// flags (Unix only — this fixture is not expected to be driven on non-Unix targets, since
/// R-SA-059's own signal-escalation ladder is itself Unix-real/non-Unix-best-effort). Errors
/// installing a handler are logged to stderr (never protocol data, R-SA-046) and otherwise
/// ignored — a fixture that fails to install its OWN test-scripted signal handler should still
/// run its steps and exit normally rather than aborting, so the resulting test failure is "the
/// signal wasn't actually ignored" (a clear, diagnosable assertion failure) rather than "the
/// fixture itself crashed" (a confusing one).
#[cfg(unix)]
async fn install_signal_ignoring(script: &FixtureScript) {
    use tokio::signal::unix::{SignalKind, signal};

    if script.ignore_sigint {
        match signal(SignalKind::interrupt()) {
            Ok(mut stream) => {
                tokio::spawn(async move {
                    loop {
                        stream.recv().await;
                        // Swallow every SIGINT delivered — never act on it.
                    }
                });
            }
            Err(err) => eprintln!("cyrup-subagent-fixture: failed to ignore SIGINT: {err}"),
        }
    }
    if script.ignore_sigterm {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                tokio::spawn(async move {
                    loop {
                        stream.recv().await;
                        // Swallow every SIGTERM delivered — never act on it. SIGKILL (not
                        // installable as a handler on any conforming OS) remains the only way to
                        // terminate a fixture process scripted this way, which is exactly the
                        // point (A-SA-11's kill-escalation proof).
                    }
                });
            }
            Err(err) => eprintln!("cyrup-subagent-fixture: failed to ignore SIGTERM: {err}"),
        }
    }
}

#[cfg(not(unix))]
async fn install_signal_ignoring(_script: &FixtureScript) {
    // No direct SIGINT/SIGTERM process-group equivalent on non-Unix targets — mirrors
    // `spawn::signal`'s own module-level documented fallback stance; this fixture simply has
    // nothing to install here, matching the real spawn boundary's own best-effort carve-out.
}

#[tokio::main]
async fn main() {
    let script = load_script();
    install_signal_ignoring(&script).await;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if script.echo_argv {
        emit_argv_echo(&mut out);
    }
    if !script.echo_env.is_empty() {
        emit_env_echo(&script.echo_env, &mut out);
    }

    for step in &script.steps {
        match step {
            ScriptStep::Emit { line } => {
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
            }
            ScriptStep::EmitStderr { line } => {
                let _ = writeln!(std::io::stderr(), "{line}");
                let _ = std::io::stderr().flush();
            }
            ScriptStep::EmitPadded {
                head,
                pad_bytes,
                tail,
            } => {
                let _ = out.write_all(head.as_bytes());
                let chunk = vec![b'x'; 64 * 1024];
                let mut written = 0usize;
                while written < *pad_bytes {
                    let take = chunk.len().min(*pad_bytes - written);
                    let _ = out.write_all(chunk.get(..take).unwrap_or_default());
                    written += take;
                }
                let _ = out.write_all(tail.as_bytes());
                let _ = out.write_all(b"\n");
                let _ = out.flush();
            }
            ScriptStep::EmitStderrPadded {
                head,
                pad_bytes,
                tail,
            } => {
                // Unbuffered, straight at the fd: every `write_all` here is a real `write(2)` the
                // OS can block, which is exactly the condition being scripted.
                let stderr = std::io::stderr();
                let mut err = stderr.lock();
                let _ = err.write_all(head.as_bytes());
                let chunk = vec![b'x'; 64 * 1024];
                let mut written = 0usize;
                while written < *pad_bytes {
                    let take = chunk.len().min(*pad_bytes - written);
                    let _ = err.write_all(chunk.get(..take).unwrap_or_default());
                    written += take;
                }
                let _ = err.write_all(tail.as_bytes());
                let _ = err.write_all(b"\n");
                let _ = err.flush();
            }
            ScriptStep::SleepMs { ms } => {
                tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
            }
            ScriptStep::WriteStructuredOutput { value } => {
                write_structured_output_capture(value);
            }
        }
    }

    let _ = out.flush();
    drop(out);
    std::process::exit(script.exit_code);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn fixture_script_default_is_empty_and_exits_zero() {
        let script = FixtureScript::default();
        assert!(script.steps.is_empty());
        assert!(!script.echo_argv);
        assert!(script.echo_env.is_empty());
        assert_eq!(script.exit_code, 0);
    }

    #[test]
    fn fixture_script_parses_emit_and_sleep_steps() {
        let json = serde_json::json!({
            "steps": [
                {"kind": "emit", "line": "{\"type\":\"agent_start\"}"},
                {"kind": "sleep_ms", "ms": 50},
                {"kind": "emit", "line": "{\"type\":\"agent_end\"}"}
            ],
            "exit_code": 3
        })
        .to_string();
        let script: FixtureScript = serde_json::from_str(&json).expect("parses");
        assert_eq!(script.steps.len(), 3);
        assert_eq!(script.exit_code, 3);
        assert!(matches!(script.steps[1], ScriptStep::SleepMs { ms: 50 }));
    }

    #[test]
    fn fixture_script_parses_signal_ignoring_flags() {
        let json = serde_json::json!({"ignore_sigint": true, "ignore_sigterm": true}).to_string();
        let script: FixtureScript = serde_json::from_str(&json).expect("parses");
        assert!(script.ignore_sigint);
        assert!(script.ignore_sigterm);
    }

    #[test]
    fn default_script_from_env_fallback_reads_echo_convenience_vars() {
        // SAFETY-equivalent note: this crate is `#![forbid(unsafe_code)]`, and `std::env::var`
        // reads (never `set_var`) are all this test performs — no environment mutation happens
        // here at all, only reading whatever this process's own test-runner environment already
        // provides, so there is no cross-test env-mutation race to worry about (unlike
        // `spawn::depth`'s/`spawn::mod`'s own documented avoidance of `set_var`).
        let script = default_script_from_env_fallback();
        // Only structural assertions: the real env in `cargo test` does not set either
        // convenience var, so this should resolve to the all-false/empty default shape.
        if std::env::var(ECHO_ARGV_ENV_VAR).is_err() {
            assert!(!script.echo_argv);
        }
        if std::env::var(ECHO_ENV_ENV_VAR).is_err() {
            assert!(script.echo_env.is_empty());
        }
    }
}

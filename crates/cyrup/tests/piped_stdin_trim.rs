//! Piped stdin must be **trimmed** before it is concatenated into the initial prompt.
//!
//! Pi's `readPipedStdin` resolves `data.trim() || undefined` (v0.83.0
//! `packages/coding-agent/src/main.ts`, inside `readPipedStdin`), so the stdin content that reaches
//! `buildInitialMessage` is already trimmed. `buildInitialMessage` then does
//! `parts.push(stdinContent)` … `parts.join("")` (v0.83.0 `cli/initial-message.ts`) — a DELIBERATE
//! empty separator that is only correct because the trim already happened upstream of it.
//!
//! cyrup's `read_piped_stdin` tested `buf.trim().is_empty()` but returned the untrimmed `buf`, so
//! `echo context | cyrup "summarise this"` sent `"context\nsummarise this"` where pi sends
//! `"contextsummarise this"`, and leading whitespace survived at the front of the prompt too.
//!
//! Proved at the BINARY seam, which is the only place the literal prompt bytes handed to the model
//! are observable: `--mode json` echoes the composed user message as JSONL on stdout.
//!
//! Fully offline: `--offline`, the scripted faux model, a tempdir HOME/agent dir, every provider key
//! and proxy scrubbed from the child env. No network, no credentials, no paid tokens.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// Run the real `cyrup` binary in a hermetic tempdir with the offline faux model, feeding `stdin`
/// through a real pipe (so `read_piped_stdin`'s non-TTY branch is the one under test), and return
/// the text of the FIRST user message the JSONL stream reports.
///
/// Holds the child's stdin open until the whole payload is written and only then closes it, so a
/// contended box cannot truncate the pipe; the child is fully awaited via `wait_with_output`.
fn user_prompt(stdin: &str, args: &[&str]) -> String {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_cyrup"))
        .current_dir(&work)
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
        // attaches intercom and detaches a real `__intercom-broker` that never self-exits
        // (`schedule_shutdown_check` is armed only by a REGISTERED session's disconnect, 1:1 with
        // pi-intercom `broker/broker.ts:221`/`:429`, and this one-shot child exits before its
        // connect task registers). That matters most HERE: `wait_with_output()` below reads to EOF,
        // not to child exit, so any surviving grandchild holding this harness's pipe deadlocks the
        // test — the observed whole-suite hang. Measured: 13 immortal brokers per run across this
        // crate's four binary-seam targets, 0 under `env -u CYRUP_INTERCOM`.
        .env_remove("CYRUP_INTERCOM")
        .env_remove("CYRUP_SUBAGENTS")
        .env_remove("CYRUP_PERMISSION_SYSTEM")
        .args([
            "--offline",
            "--no-session",
            "--no-extensions",
            "--model",
            "faux/faux-1",
            "--mode",
            "json",
        ])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cyrup");

    {
        let mut pipe = child.stdin.take().expect("child stdin");
        pipe.write_all(stdin.as_bytes()).expect("write stdin");
        pipe.flush().expect("flush stdin");
        // Dropping the handle here is the EOF the child's `read_to_string` is waiting for.
    }

    let out = child.wait_with_output().expect("await cyrup");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    // Drain the JSONL stream to the first `message_start` carrying a user message, rather than
    // indexing a fixed line — the header/agent_start prelude is not this test's contract.
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("message_start") {
            continue;
        }
        let message = value.get("message").unwrap_or(&serde_json::Value::Null);
        if message.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let text = message
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_else(|| panic!("user message has no text block: {line}"));
        return text.to_string();
    }
    panic!("no user message in the JSONL stream.\nstdout was:\n{stdout}\nstderr was:\n{stderr}");
}

/// THE headline: stdin arrives trimmed, so the empty `parts.join("")` separator produces pi's exact
/// bytes. Before the fix this was `"context\nsummarise this"`.
#[test]
fn piped_stdin_is_trimmed_before_it_joins_the_message() {
    assert_eq!(
        user_prompt("context\n", &["summarise this"]),
        "contextsummarise this",
        "pi trims stdin in readPipedStdin (main.ts) and then joins with \"\" \
         (initial-message.ts), so no separator byte may survive"
    );
}

/// Leading whitespace is trimmed too — pi's `data.trim()` is two-sided, so the divergence at the
/// FRONT of the prompt closes with the same change.
#[test]
fn leading_whitespace_on_piped_stdin_is_trimmed() {
    assert_eq!(
        user_prompt("\n\t  context  \n\n", &["summarise this"]),
        "contextsummarise this"
    );
}

/// Stdin alone, with no CLI message: the prompt is the trimmed stdin and nothing else.
#[test]
fn piped_stdin_alone_is_the_trimmed_prompt() {
    assert_eq!(user_prompt("  just context  \n", &[]), "just context");
}

/// MIRROR (green before AND after the fix): stdin that carries no surrounding whitespace is already
/// in its trimmed form, so the concatenation is unchanged by the trim. This is what shows the
/// assertions above are reading the real prompt bytes rather than passing vacuously.
#[test]
fn mirror_already_trimmed_stdin_concatenates_unchanged() {
    assert_eq!(
        user_prompt("context", &["summarise this"]),
        "contextsummarise this"
    );
}

/// MIRROR (green before AND after the fix): whitespace-only stdin contributes NO part at all — the
/// emptiness test was already on the trimmed value, and that behavior must not change.
#[test]
fn mirror_whitespace_only_stdin_contributes_nothing() {
    assert_eq!(user_prompt("   \n\t\n", &["summarise this"]), "summarise this");
}

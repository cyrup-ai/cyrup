//! SEAM-135 — `--list-models` runs AFTER runtime creation, so it behaves like `--help`: pi's
//! `main.ts:866-871` @v0.87.1 sits straight after the `--help` exit (`:857-864`), ahead of the
//! diagnostics checkpoint (`:896`), on an in-memory session (`createSessionManager`, `:363`).
//! Driven through the shipped binary in a cleared environment, with a stored `groq` key as the only
//! credential so the row assertions do not depend on the developer's exports.
//!
//! The extension-registered-provider row, which needs a live session, is asserted in
//! `src/tests/list_models_session.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fx {
    tmp: tempfile::TempDir,
    agent: PathBuf,
    cwd: PathBuf,
}

fn fixture(stored: &[&str]) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let creds: serde_json::Map<String, serde_json::Value> = stored
        .iter()
        .map(|id| {
            (
                (*id).to_string(),
                serde_json::json!({"type": "api_key", "key": format!("sk-{id}")}),
            )
        })
        .collect();
    std::fs::write(
        agent.join("auth.json"),
        serde_json::to_string(&creds).unwrap(),
    )
    .unwrap();
    Fx { tmp, agent, cwd }
}

fn run(fx: &Fx, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.args(args).current_dir(&fx.cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", fx.tmp.path())
        .env("CYRUP_AGENT_DIR", &fx.agent)
        .env("CYRUP_OFFLINE", "1");
    cmd.output().expect("spawning the cyrup binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn files_named(dir: &Path, ext: &str, into: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_named(&path, ext, into);
        } else if path.extension().is_some_and(|e| e == ext) {
            into.push(path);
        }
    }
}

/// Baseline: the stored credential lists its provider, and nothing else.
#[test]
fn a_stored_credential_provider_is_listed() {
    let fx = fixture(&["groq"]);
    let out = run(&fx, &["--list-models"]);
    assert!(out.status.success(), "stderr:\n{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.lines().next().unwrap().starts_with("provider"),
        "{text}"
    );
    assert!(
        text.lines().skip(1).all(|l| l.starts_with("groq")),
        "{text}"
    );
    assert!(text.lines().count() > 1, "{text}");
}

/// An unknown `--provider` is a runtime diagnostic (`main.ts:468-479`), reported only after the
/// exits — it must not stop the listing.
#[test]
fn an_unknown_provider_does_not_gate_the_listing() {
    let fx = fixture(&["groq"]);
    let out = run(&fx, &["--list-models", "--provider", "bogus"]);
    assert!(out.status.success(), "stderr:\n{}", stderr(&out));
    assert!(stdout(&out).contains("groq"), "{}", stdout(&out));
}

/// `--api-key` without a model is a runtime diagnostic too (`main.ts:810-818`).
#[test]
fn api_key_without_a_model_still_lists() {
    let fx = fixture(&["groq"]);
    let out = run(&fx, &["--api-key", "k", "--list-models"]);
    assert!(out.status.success(), "stderr:\n{}", stderr(&out));
    assert!(stdout(&out).contains("groq"), "{}", stdout(&out));
    assert!(
        !stderr(&out).contains("--api-key requires"),
        "stderr:\n{}",
        stderr(&out)
    );
}

/// SEAM-147 — the same diagnostic, outside a listing, reads the RESOLVED model (pi `main.ts:830-834`
/// @f1b2e77f5): `--models ""` resolves no scope, so `--api-key` has no model and the run exits 1
/// with pi's error, not with the no-models guidance it reached when only the flag's presence was
/// checked.
#[test]
fn api_key_with_an_empty_models_scope_is_refused() {
    let fx = fixture(&["groq"]);
    let out = run(&fx, &["--api-key", "k", "--models", "", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "Error: --api-key requires a model to be specified via --model, --provider/--model, or --models"
        ),
        "stderr:\n{}",
        stderr(&out)
    );
}

/// The in-memory session (`main.ts:363`): a missing `--session` neither fails the run nor writes a
/// session file.
#[test]
fn a_missing_session_is_neither_looked_up_nor_written() {
    let fx = fixture(&["groq"]);
    let out = run(&fx, &["--list-models", "--session", "no-such"]);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", stderr(&out));
    assert!(stdout(&out).contains("groq"), "{}", stdout(&out));
    let mut jsonl = Vec::new();
    files_named(fx.tmp.path(), "jsonl", &mut jsonl);
    assert!(jsonl.is_empty(), "a listing wrote session files: {jsonl:?}");
}

/// `getAvailable()` filters by auth alone. `--provider anthropic` installs anthropic's provider on
/// the session, but with no anthropic credential its models are not available.
#[test]
fn an_installed_but_uncredentialed_provider_is_not_listed() {
    let fx = fixture(&[]);
    let out = run(&fx, &["--list-models", "--provider", "anthropic"]);
    assert!(out.status.success(), "stderr:\n{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("No models available."),
        "{}",
        stdout(&out)
    );
}

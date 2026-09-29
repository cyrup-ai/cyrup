//! CFG-088 — a broken settings file is reported ONCE, in pi's shape, naming the file.
//!
//! pi builds two settings managers — the startup one (`main.ts:657`) and the session's own
//! (`main.ts:785`) — and both read the same broken global `settings.json`. It renders each error as
//! `Invalid settings file <path>: <msg>` (`core/settings-diagnostics.ts:4-9` @v0.87.1), merges the
//! two lists with `deduplicateDiagnostics`, and prints the result for a non-interactive run
//! (`main.ts:896-900`). Driven through the shipped binary, in a cleared environment so the broken
//! file is the only settings source and no ambient credential selects a model.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Command;

#[test]
fn a_broken_global_settings_file_is_one_warning_naming_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let settings = agent.join("settings.json");
    std::fs::write(&settings, "{ not json").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.args(["-p", "hi"]).current_dir(&cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", tmp.path())
        .env("CYRUP_AGENT_DIR", &agent)
        .env("CYRUP_OFFLINE", "1");
    let out = cmd.output().expect("spawning the cyrup binary");
    let stderr = String::from_utf8_lossy(&out.stderr);

    let prefix = format!("Warning: Invalid settings file {}: ", settings.display());
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("settings"))
        .filter(|l| l.starts_with("Warning:"))
        .collect();
    assert_eq!(warnings.len(), 1, "stderr:\n{stderr}");
    assert!(warnings[0].starts_with(&prefix), "stderr:\n{stderr}");
}

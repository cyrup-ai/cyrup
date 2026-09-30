//! CFG-088 / SEAM-020 — `cyrup --help` is answered where pi answers it: after the runtime exists.
//!
//! pi `main.ts:857-864` @v0.87.1, downstream of `createAgentSessionRuntime` (`:845`):
//!
//! ```ts
//! if (parsed.help) {
//!     reportDiagnostics(startupSettingsDiagnostics);
//!     const extensionFlags = resourceLoader.getExtensions()
//!         .extensions.flatMap((extension) => Array.from(extension.flags.values()));
//!     printHelp(extensionFlags);
//!     process.exit(0);
//! }
//! ```
//!
//! cyrup printed `render_help(&[])` before its dirs were even resolved, so the help never listed a
//! loaded extension's flag and never carried the startup settings diagnostics. Both halves are
//! only observable on the real binary's stdout and stderr, so this spawns it: the always-loaded
//! MCP adapter declares `--mcp-config` (`registerFlag("mcp-config", { description: "Path to MCP
//! config file", type: "string" })`), and a malformed global `settings.json` must produce pi's
//! warning — with `JSON.parse`'s own message, as Node 22 words it for this document.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Stdio;

use tempfile::TempDir;

/// A trailing comma. Node v22's `JSON.parse` rejects it with the message below.
const MALFORMED: &str = "{\n  \"theme\": \"dark\",\n}\n";
const JSON_PARSE_MESSAGE: &str =
    "Expected double-quoted property name in JSON at position 21 (line 3 column 1)";

#[test]
fn help_lists_extension_flags_after_the_startup_settings_diagnostics() {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let settings = agent_dir.join("settings.json");
    std::fs::write(&settings, MALFORMED).unwrap();

    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), tmp.path());
    cmd.current_dir(&work)
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .args(["--offline", "--help"])
        .stdin(Stdio::null());
    let out = cmd.output().expect("spawn cyrup");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{stderr}");

    // `reportDiagnostics(startupSettingsDiagnostics)` — once, naming the file, with pi's message.
    let warning = format!(
        "Warning: Invalid settings file {}: {JSON_PARSE_MESSAGE}",
        settings.display()
    );
    let settings_warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("Invalid settings file"))
        .collect();
    assert_eq!(
        settings_warnings,
        vec![warning.as_str()],
        "stderr:\n{stderr}"
    );

    // `printHelp(extensionFlags)` — the loaded extension's declaration, in pi's row shape
    // (`` `  --${name}${value}`.padEnd(30) + description ``, `cli/args.ts:262-270`).
    let block = stdout
        .split("Extension CLI Flags:\n")
        .nth(1)
        .unwrap_or_else(|| panic!("no extension flag block in --help:\n{stdout}"));
    assert!(
        block
            .lines()
            .any(|l| l == "  --mcp-config <value>        Path to MCP config file"),
        "--help:\n{stdout}"
    );
    assert!(
        stdout.starts_with("cyrup - AI coding assistant"),
        "{stdout}"
    );
}

/// pi's `createSessionManager` answers `--help` with `SessionManager.inMemory(cwd, …)` before it
/// looks at `--session` / `--fork` / `--resume` (`main.ts:363-365` @v0.87.1), so a session
/// reference that resolves to nothing does not stop the help, and nothing is written.
///
/// **Red without the fix:** cyrup resolved `--session` first and exited 1 with its "no session"
/// error before the runtime (and so the help) was ever reached.
#[test]
fn help_runs_on_an_in_memory_session() {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&work).unwrap();

    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), tmp.path());
    cmd.current_dir(&work)
        .env("CYRUP_AGENT_DIR", &agent_dir)
        .args(["--offline", "--help", "--session", "no-such-session"])
        .stdin(Stdio::null());
    let out = cmd.output().expect("spawn cyrup");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{stderr}");
    assert!(
        stdout.starts_with("cyrup - AI coding assistant"),
        "{stdout}"
    );
    assert!(stdout.contains("Extension CLI Flags:"), "{stdout}");
    let sessions = agent_dir.join("sessions");
    let written = std::fs::read_dir(&sessions)
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    assert_eq!(
        written,
        0,
        "a --help run wrote under {}",
        sessions.display()
    );
}

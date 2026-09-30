//! TUI-003 — the boot transcript lands in pi's order.
//!
//! pi builds it in two steps. `init()` ends with `renderInitialMessages()`
//! (`packages/coding-agent/src/modes/interactive/interactive-mode.ts:1036` @v0.87.1): the replayed
//! branch, the untrusted-project banner, then `Session compacted N times` (`:4056-4071`). Only
//! then does `run()` push the startup warnings (`:1120-1161`) and submit the CLI's initial message
//! (`:1164-1166`).
//!
//! cyrup pushed the warnings BEFORE the replay, and pushed the compaction count from `App::run`'s
//! seed — after `interactive.rs` had already pushed the initial message — so `cyrup -c "msg"` on a
//! compacted session showed the count below the user's own message. This drives the real boot
//! helper, [`crate::interactive::render_boot_transcript`], over a `--resume`d faux session and reads
//! the order off the transcript's pending entries, which is what the first frame commits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use cyrup_tui::{App, Entry, UiTheme};
use ratatui::backend::TestBackend;

use crate::diagnostics::Diagnostic;
use crate::interactive::render_boot_transcript;

/// A pi session file with one message, two compactions (one on an abandoned branch — pi's
/// `getEntries()` counts both), and a user message after the kept one.
fn compacted_session(dir: &Path, cwd: &Path) -> PathBuf {
    let ts = "2026-01-01T00:00:00.000Z";
    let cwd = serde_json::to_string(&cwd.to_string_lossy()).unwrap();
    let lines = [
        format!(
            r#"{{"type":"session","version":3,"id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0003","timestamp":"{ts}","cwd":{cwd}}}"#
        ),
        format!(
            r#"{{"type":"message","id":"u1","parentId":null,"timestamp":"{ts}","message":{{"role":"user","content":"first question","timestamp":1}}}}"#
        ),
        format!(
            r#"{{"type":"compaction","id":"x0","parentId":"u1","timestamp":"{ts}","summary":"abandoned","firstKeptEntryId":"u1","tokensBefore":100}}"#
        ),
        format!(
            r#"{{"type":"compaction","id":"c1","parentId":"u1","timestamp":"{ts}","summary":"kept","firstKeptEntryId":"u1","tokensBefore":100}}"#
        ),
        format!(
            r#"{{"type":"message","id":"u2","parentId":"c1","timestamp":"{ts}","message":{{"role":"user","content":"after the compaction","timestamp":2}}}}"#
        ),
    ];
    let path = dir.join("resumed.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

/// One label per pending entry the boot pushed, in order.
fn labels(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::User { text, .. } => Some(format!("user|{text}")),
            Entry::Status(s) => Some(format!("status|{s}")),
            Entry::Warning(w) => Some(format!("warning|{w}")),
            _ => None,
        })
        .collect()
}

/// `cyrup -c "msg"` on a compacted session: replay → `Session compacted 2 times` → the startup
/// diagnostics → the migrated-credential notice → the model-fallback warning → the initial message.
///
/// **Red without the fix:** the startup warnings preceded the replay, and the count was not pushed
/// here at all (it came from `App::run`'s seed, after the initial message).
#[tokio::test]
async fn a_continued_compacted_session_boots_in_pis_order() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd.clone(), agent_dir);
    config.trust_override = Some(true);
    let path = compacted_session(tmp.path(), &cwd);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(SessionFactory::new(provider, config));
    let rt = AgentSessionRuntime::create(factory, SessionTarget::Resume(path))
        .await
        .unwrap();
    let session = rt.session().await;

    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    render_boot_transcript(
        &mut app,
        &session,
        &[Diagnostic::warning(
            "Invalid settings file /a/settings.json: bad",
        )],
        &["anthropic".to_string()],
        Some("No models available.".to_string()),
        Some("msg"),
    )
    .await;

    let got = labels(&app);
    let pos = |needle: &str| {
        got.iter()
            .position(|l| l == needle)
            .unwrap_or_else(|| panic!("no `{needle}` in {got:#?}"))
    };
    let order = [
        pos("user|after the compaction"),
        pos("status|Session compacted 2 times"),
        pos("warning|Warning: Invalid settings file /a/settings.json: bad"),
        pos("warning|Warning: Migrated credentials to auth.json: anthropic"),
        pos("warning|Warning: No models available."),
        pos("user|msg"),
    ];
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "boot transcript out of pi's order: {got:#?}"
    );
}

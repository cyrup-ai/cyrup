//! The `/resume` listing seam must read the session's OWN directory, not the cwd-encoded default.
//!
//! Pi's in-TUI picker calls `SessionManager.list(this.sessionManager.getCwd(),
//! this.sessionManager.getSessionDir())` (interactive-mode.ts:4867), and `getSessionDir()` returns
//! the directory fixed at manager construction (`session-manager.ts:999-1001`): an explicit
//! `--session-dir` verbatim (`create`, :1519-1520). `SessionManager.list` then applies
//! `filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)` (:1639-1643), so
//! a custom directory pooling several projects only shows the current cwd's sessions.
//!
//! Re-deriving `<agent_dir>/sessions/--<encoded-cwd>--` instead leaves the picker blind under
//! `--session-dir` — the live session is not even in its own list.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

fn faux() -> Arc<FauxProvider> {
    let f = Arc::new(FauxProvider::new());
    f.set_responses(vec![faux_assistant_message(vec![faux_text("an answer")], StopReason::Stop)]);
    f
}

/// Under `--session-dir` the picker lists the custom directory: the live session's own file is
/// there, and the (empty) cwd-encoded default is never consulted.
#[tokio::test]
async fn list_sessions_reads_the_explicit_session_dir() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let custom = tmp.path().join("custom-sessions");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let provider: Arc<dyn Provider> = faux();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.session_dir = Some(custom.clone()); // explicit --session-dir
    let session = SessionBuilder::new(provider, cfg).build().await.expect("build");

    let _ = session.prompt("port the editor").await.expect("prompt");
    session.wait_for_idle().await;

    let file = session.session_file().await.expect("persisted file");
    assert_eq!(file.parent().unwrap(), custom, "precondition: the file is in the custom dir");
    assert_eq!(session.session_dir(), custom, "getSessionDir() is the explicit dir");

    let paths: Vec<PathBuf> = session.list_sessions().iter().map(|s| s.path.clone()).collect();
    assert!(
        paths.contains(&file),
        "the /resume picker must list the session's own dir; got {paths:#?}"
    );
}

/// A custom `--session-dir` may pool several projects' sessions in one flat directory, so the local
/// listing keeps only this cwd's (Pi `filterCwd`, session-manager.ts:1639-1643). The foreign file
/// here is a well-formed session whose header `cwd` is a DIFFERENT project.
#[tokio::test]
async fn list_sessions_filters_a_shared_dir_by_cwd() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let other_cwd = tmp.path().join("other-project");
    let agent_dir = tmp.path().join("agent");
    let custom = tmp.path().join("shared-sessions");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&other_cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&custom).unwrap();

    // Another project's session, dropped into the same shared directory.
    let foreign = custom.join("2026-01-01T00-00-00-000Z_0193f0e1-0000-7000-8000-0000000000ff.jsonl");
    std::fs::write(
        &foreign,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "0193f0e1-0000-7000-8000-0000000000ff",
                "timestamp": "2026-01-01T00:00:00.000Z",
                "cwd": other_cwd.display().to_string(),
            })
        ),
    )
    .unwrap();

    let provider: Arc<dyn Provider> = faux();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.session_dir = Some(custom.clone());
    let session = SessionBuilder::new(provider, cfg).build().await.expect("build");

    let _ = session.prompt("port the editor").await.expect("prompt");
    session.wait_for_idle().await;

    let file = session.session_file().await.expect("persisted file");
    let paths: Vec<PathBuf> = session.list_sessions().iter().map(|s| s.path.clone()).collect();
    assert!(paths.contains(&file), "this project's session must be listed; got {paths:#?}");
    assert!(
        !paths.contains(&foreign),
        "another project's session in a shared --session-dir must be filtered out; got {paths:#?}"
    );
}

/// Without `--session-dir` nothing changes: the session dir IS the cwd-encoded default, so the
/// listing is unfiltered and shows this cwd's sessions under `<agent_dir>/sessions`.
#[tokio::test]
async fn list_sessions_still_uses_the_encoded_default_dir() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let provider: Arc<dyn Provider> = faux();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(provider, cfg).build().await.expect("build");

    let _ = session.prompt("port the editor").await.expect("prompt");
    session.wait_for_idle().await;

    let file = session.session_file().await.expect("persisted file");
    assert!(
        session.session_dir().starts_with(agent_dir.join("sessions")),
        "default dir lives under the sessions root: {:?}",
        session.session_dir()
    );
    let paths: Vec<PathBuf> = session.list_sessions().iter().map(|s| s.path.clone()).collect();
    assert!(paths.contains(&file), "the default listing must still find the session; got {paths:#?}");
}

//! WHERE a session lands on disk decides whether the `/resume` listing can see it. Two seams get
//! that wrong in the same way: the listing itself re-deriving a directory instead of reading the
//! session's own, and `import_from_jsonl` copying into the sessions ROOT instead of the live
//! session's per-cwd directory. Both leave a real session invisible to every listing path.
//!
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

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::{
    AgentSessionRuntime, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
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

// ----------------------------------------------------------------------- import_from_jsonl ----

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: `import_from_jsonl` — the imported transcript becomes the ACTIVE session.
#[tokio::test]
async fn runtime_import_from_jsonl_switches_session() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("imported")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();

    // Build a source session with content and export it to a standalone JSONL file.
    let source = SessionBuilder::new(provider.clone(), base_config(&fx)).build().await.unwrap();
    let _ = source.prompt("seed message").await.unwrap();
    source.wait_for_idle().await;
    let export_path = fx.cwd.join("exported.jsonl");
    source.export_to_jsonl(Some(&export_path)).await.unwrap();
    assert!(export_path.exists());
    drop(source);

    // A fresh runtime imports the file and switches to it (not cancelled).
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();
    let result = runtime.import_from_jsonl(export_path, None).await.expect("import");
    assert!(!result.cancelled, "import must not be cancelled");
    let imported = runtime.session().await;
    let texts: Vec<String> = imported
        .messages()
        .await
        .iter()
        .filter_map(|m| match m {
            cyrup_core::Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "seed message"), "imported transcript missing: {texts:?}");

    // A missing source path is a typed error, not a panic.
    match runtime.import_from_jsonl(fx.cwd.join("nope.jsonl"), None).await {
        Err(crate::SessionServiceError::ImportFileNotFound(_)) => {}
        other => panic!("expected ImportFileNotFound, got {other:?}"),
    }
}

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: `import_from_jsonl`, second half — WHERE the imported file lands.
///
/// Pi `importFromJsonl` copies into `this.session.sessionManager.getSessionDir()`
/// (agent-session-runtime.ts:367) — the ACTIVE session's own per-cwd directory
/// (`<root>/--<enc-cwd>--`, session-manager.ts:484,999-1000), never the sessions ROOT. Landing it in
/// the root leaves the imported session invisible to every listing path: `listing::list` scans
/// `layout.dir()` and `list_all` only descends into per-project subdirectories.
#[tokio::test]
async fn runtime_import_lands_in_the_per_cwd_session_dir_and_is_listable() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();

    // Source transcript exported to a standalone file outside the sessions tree.
    let source = SessionBuilder::new(provider.clone(), base_config(&fx)).build().await.unwrap();
    let _ = source.prompt("seed message").await.unwrap();
    source.wait_for_idle().await;
    let export_path = fx.cwd.join("exported.jsonl");
    source.export_to_jsonl(Some(&export_path)).await.unwrap();
    drop(source);

    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();

    // The live session's own directory — the per-cwd `--<enc-cwd>--` dir, one level below the root.
    let sessions_root = fx.agent_dir.join("sessions");
    let live_file = runtime.session().await.session_file().await.expect("persisted session");
    let per_cwd_dir = live_file.parent().expect("session file has a parent").to_path_buf();
    assert_ne!(
        per_cwd_dir, sessions_root,
        "fixture precondition: the default layout must nest a per-cwd dir under the root"
    );

    let result = runtime.import_from_jsonl(&export_path, None).await.expect("import");
    assert!(!result.cancelled);

    // The copy lands beside the live session, NOT in the sessions root.
    assert!(
        per_cwd_dir.join("exported.jsonl").exists(),
        "import must copy into the per-cwd session dir {}",
        per_cwd_dir.display()
    );
    assert!(
        !sessions_root.join("exported.jsonl").exists(),
        "import must NOT copy into the sessions root {}",
        sessions_root.display()
    );
    // ...and the switched-to session is the copy in that dir.
    let switched = runtime.session().await.session_file().await.expect("imported session file");
    assert_eq!(switched, per_cwd_dir.join("exported.jsonl"));

    // Consequence Pi relies on: the imported session is visible to both listing paths.
    let listed = cyrup_session::listing::list_in_dir(&per_cwd_dir, None, None);
    assert!(
        listed.iter().any(|s| s.path == per_cwd_dir.join("exported.jsonl")),
        "imported session missing from the per-cwd listing: {:?}",
        listed.iter().map(|s| s.path.clone()).collect::<Vec<_>>()
    );
    let all = cyrup_session::listing::list_all(&cyrup_session::layout::SessionsRoot(sessions_root));
    assert!(
        all.iter().any(|s| s.path == per_cwd_dir.join("exported.jsonl")),
        "imported session missing from the cross-project listing: {:?}",
        all.iter().map(|s| s.path.clone()).collect::<Vec<_>>()
    );
}

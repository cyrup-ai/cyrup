//! Branch/`--session-dir` directory parity, driven end to end over the ASSEMBLED session service
//! (`SessionBuilder` → `AgentSession`, real scripted `FauxProvider`). Pins gap-analysis 05 Findings 1
//! & 3 against regression:
//!   * a fork/clone reuses the open session's OWN directory (Pi `createBranchedSession` reuses
//!     `this.getSessionDir()`, session-manager.ts:918-920,1343) instead of re-encoding it one level
//!     deeper — so the branch stays visible to the `/resume` listing;
//!   * an explicit `--session-dir` is used LITERALLY (Pi `sessionDir ? normalizePath(sessionDir) :
//!     getDefaultSessionDir(cwd)`, session-manager.ts:1430) rather than gaining a `--<cwd>--` subdir.
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
    f.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    f
}

#[tokio::test]
async fn finding1_clone_stays_in_same_dir_and_is_listed() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let provider: Arc<dyn Provider> = faux();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(provider, cfg).build().await.expect("build");

    let _ = session.prompt("port the editor").await.expect("prompt 1");
    session.wait_for_idle().await;

    let file_before = session.session_file().await.expect("persisted file");
    let dir_before = file_before.parent().unwrap().to_path_buf();
    println!("F1 dir_before = {}", dir_before.display());

    // Clone at the current leaf → a NEW branched file. Must land in the SAME directory (Pi reuses
    // this.sessionDir), not one --enc-- level deeper.
    session.clone_at(None).await.expect("clone_at");
    let file_after = session.session_file().await.expect("branched file");
    let dir_after = file_after.parent().unwrap().to_path_buf();
    println!("F1 dir_after  = {}", dir_after.display());
    assert_eq!(dir_after, dir_before, "Finding 1: branch must stay in the same dir");
    assert_ne!(file_after, file_before, "branch is a distinct file");

    // The /resume listing seam re-derives a fresh single-level layout from the sessions root; the
    // branched file must be visible there (it was orphaned before the fix).
    let listed = session.list_sessions();
    let paths: Vec<PathBuf> = listed.iter().map(|s| s.path.clone()).collect();
    println!("F1 listed = {paths:#?}");
    assert!(paths.contains(&file_before), "original must be listed");
    assert!(paths.contains(&file_after), "Finding 1: branched session must be visible to --resume");
}

#[tokio::test]
async fn finding3_explicit_session_dir_is_literal() {
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

    let _ = session.prompt("hi").await.expect("prompt");
    session.wait_for_idle().await;

    let file = session.session_file().await.expect("persisted file");
    let dir = file.parent().unwrap().to_path_buf();
    println!("F3 file = {}", file.display());
    // Pi: <custom>/<ts>_<id>.jsonl. Buggy: <custom>/--...--/<ts>_<id>.jsonl (one too deep).
    assert_eq!(dir, custom, "Finding 3: explicit --session-dir must be used literally");
}

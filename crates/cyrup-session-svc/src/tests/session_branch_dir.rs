//! Branch/`--session-dir` directory parity, driven end to end over the ASSEMBLED session service
//! (`SessionBuilder` → `AgentSession`, real scripted `FauxProvider`). Pins gap-analysis 05 Findings 1
//! & 3 against regression:
//!   * a fork/clone reuses the open session's OWN directory (Pi `createBranchedSession` reuses
//!     `this.getSessionDir()`, session-manager.ts:918-920,1343) instead of re-encoding it one level
//!     deeper — so the branch stays visible to the `/resume` listing;
//!   * an explicit `--session-dir` is used LITERALLY (Pi `sessionDir ? normalizePath(sessionDir) :
//!     getDefaultSessionDir(cwd)`, session-manager.ts:1430) rather than gaining a `--<cwd>--` subdir.
//!
//! The `clone_at` half of the same seam is here too: a clone must write a NEW file beside the
//! original rather than mutating it in place.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::{SessionBuilder, SessionConfig, SessionFactory, SessionTarget};
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

// ------------------------------------------------------- clone_at + runtime fallback getter ----

/// Facade parity vs Pi `agent-session.ts`, two items: `clone_at` writes a NEW session file rather than mutating the
/// original, and the runtime's `modelFallbackMessage` getter surfaces the message a fallback
/// resolution left behind.
#[tokio::test]
async fn clone_at_creates_new_file_and_runtime_surfaces_fallback() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider.clone(), base_config(&fx)).build().await.unwrap();

    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let _ = session.prompt("hi").await.unwrap();
    session.wait_for_idle().await;

    let original = session.session_id().clone();
    let cloned = session.clone_at(None).await.unwrap();
    assert_ne!(cloned, original, "clone_at branches into a distinct session id");

    // Runtime re-surfaces the (absent) model-fallback message of its active session.
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = crate::AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap();
    assert!(runtime.model_fallback_message().await.is_none(), "clean model resolve = no fallback");
}

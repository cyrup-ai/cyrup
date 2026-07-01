//! Feature #2 — the flat session-DAG getter (`AgentSession::session_dag`), proven over a REAL
//! multi-branch `AgentSession` driven by the scripted `FauxProvider`. The `/tree` selector's
//! connector/fold engine was data-starved (no flat-DAG getter → a flat user-message list); this getter
//! walks the manager's real branch tree into pre-order `SessionDagNode`s with parent/depth/label/kind/
//! fold/leaf/label/timestamp (Pi `flattenTree` over `SessionManager.getTree()`, `tree-selector.ts`).
//!
//! We build a real fork: prompt once, branch back to the first user message, prompt again — so the
//! first user entry gains TWO children (the original assistant reply and the new branch). The getter
//! must then report a multi-depth DAG with a foldable node, parent links, and exactly one leaf.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig, SessionDagKind};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

#[tokio::test]
async fn session_dag_flattens_a_real_multi_branch_session() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("third answer")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session =
        SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build session");

    // Turn 1 on the main line: user1 → assistant1.
    let _ = session.prompt("port the editor").await.expect("prompt 1");
    session.wait_for_idle().await;

    // Branch back to the FIRST user message, then prompt again → user1 gains a second child (a fork).
    let anchors = session.user_messages_for_forking().await;
    assert!(!anchors.is_empty(), "expected a user message to branch from");
    let first_user = anchors[0].entry_id.clone();
    session.branch(first_user.clone()).await.expect("branch to first user message");
    let _ = session.prompt("wire up streaming").await.expect("prompt 2");
    session.wait_for_idle().await;

    // The getter flattens the real branch tree.
    let dag = session.session_dag().await;

    // (>1 node) the fork produced multiple entries across multiple depths.
    assert!(dag.len() >= 4, "expected a multi-node DAG, got {}: {dag:#?}", dag.len());
    assert!(dag.iter().any(|n| n.depth > 0), "no nested node — the DAG was flattened to depth 0");
    // (fork) the branched-from user message is foldable (it now has two children). It need not be a
    // depth-0 root — real sessions carry `model_change`/`thinking_level_change` ancestor entries above
    // the first user message, so it sits deeper in the DAG (which is exactly the richer structure the
    // flat getter must preserve).
    let root = dag.iter().find(|n| n.entry_id == first_user).expect("first user node present");
    assert!(root.foldable, "the fork point must be foldable (has >1 child): {dag:#?}");
    // (parent links) non-root nodes carry a parent id.
    assert!(dag.iter().any(|n| n.parent_id.is_some()), "no parent links recorded in the flat DAG");
    // (leaf) exactly one node is the current branch leaf (the newest assistant reply).
    assert_eq!(dag.iter().filter(|n| n.is_leaf).count(), 1, "exactly one leaf expected: {dag:#?}");
    // (kinds/labels) user + assistant messages are classified and role-labeled.
    assert!(
        dag.iter().any(|n| n.kind == SessionDagKind::Message && n.label.starts_with("user:")),
        "no role-labeled user message node: {dag:#?}"
    );
    assert!(
        dag.iter().any(|n| n.label.contains("assistant:")),
        "no assistant node in the flattened DAG: {dag:#?}"
    );
    // Pre-order: the first node is a root (depth 0).
    assert_eq!(dag[0].depth, 0, "pre-order flatten must start at a root");
}

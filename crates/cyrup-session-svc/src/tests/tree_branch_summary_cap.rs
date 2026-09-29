//! SESS-060 — the `/tree` branch summary is capped at pi's `min(4096, model.maxTokens)`
//! (`branch-summarization.ts:345` @v0.87.1), not the fixed 2048 the session's own copy of the
//! request builder kept. Driven through [`crate::AgentSession::navigate_tree`] with `summarize`, the
//! path `/tree` takes, and read off the `StreamOptions` the provider receives.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use crate::{NavigateTreeOptions, SessionBuilder, SessionConfig};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use tempfile::TempDir;

#[tokio::test]
async fn the_tree_branch_summary_is_capped_at_4096_for_a_16k_output_model() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;

    let seen: Arc<Mutex<Vec<Option<u64>>>> = Arc::default();
    let step = |text: &'static str| {
        let sink = Arc::clone(&seen);
        FauxResponseStep::factory(move |_ctx, opts, _state, _model| {
            sink.lock().unwrap().push(opts.max_tokens);
            faux_assistant_message(vec![faux_text(text)], StopReason::Stop)
        })
    };
    let faux = Arc::new(FauxProvider::new());
    // The faux model reports `maxTokens: 16384`, so pi's cap is 4096.
    assert_eq!(faux.model().max_tokens, 16_384);
    faux.set_response_steps(vec![
        step("first answer"),
        step("abandoned answer"),
        step("BRANCH SUMMARY"),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build");
    let _ = session.prompt("first question").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("abandoned question").await.unwrap();
    session.wait_for_idle().await;

    let second_user = session.user_messages_for_forking().await[1]
        .entry_id
        .clone();
    let outcome = session
        .navigate_tree(
            second_user,
            NavigateTreeOptions {
                summarize: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(outcome.summary_entry.is_some(), "a summary was appended");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "two answers and one summary: {seen:?}");
    assert_eq!(seen[2], Some(4096));
}

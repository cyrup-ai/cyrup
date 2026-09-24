//! SUBA-017 §4d — the injection pump re-drains its channel AFTER `wait_for_idle`.
//!
//! The pump used to drain only before it parked on the session's idle latch. A busy parent parks
//! it for up to a whole turn, and every injection that arrived during that park was left in the
//! channel: the batch taken before the wait ran as one turn and the late arrivals paid a turn of
//! their own. This test holds the parent busy on a gated provider response, queues one injection
//! (the pump takes it and parks), queues a second while it is parked, then releases the parent.
//! Both must ride ONE injected turn.
//!
//! Killing mutation: delete the post-`wait_for_idle` `try_recv` loop in `drive_injections`
//! (`session/mod.rs`) — the provider is then called three times (prompt, c1, c2), not two.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use tempfile::TempDir;

use crate::{InputSource, SessionBuilder, SessionConfig, UserInput};

const KIND: &str = "subagent-notify";

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

/// Every persisted custom-message body of `kind`, in transcript order.
async fn persisted_bodies(session: &Arc<crate::AgentSession>) -> Vec<String> {
    use cyrup_session::agent_message::AgentMessage as Raw;
    session
        .raw_context_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            Raw::Custom(c) if c.custom_type == KIND => Some(c.content.to_string()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn injections_queued_while_the_pump_waits_for_idle_ride_one_turn() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let held = gate.clone();
    // Call 1 (the user's prompt) parks until the test releases it; every later call answers at once.
    faux.set_response_steps(vec![
        FauxResponseStep::async_factory(move |_, _, _, _| {
            let held = held.clone();
            async move {
                held.notified().await;
                faux_assistant_message(vec![faux_text("user turn done")], StopReason::Stop)
            }
        }),
        faux_assistant_message(vec![faux_text("saw the completions")], StopReason::Stop).into(),
        faux_assistant_message(vec![faux_text("a second injected turn")], StopReason::Stop).into(),
    ]);
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap()
        .into_shared();

    let _stream = session
        .prompt(UserInput::text("start", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    // The run is live and blocked inside the provider.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while faux.call_count() < 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the prompt never reached the provider"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    session
        .inject_message("c1".into(), Some(KIND.into()), false, None, true)
        .await
        .unwrap();
    // Give the pump time to take c1 and park on the busy session's idle latch. (If it has not yet
    // parked, c1 and c2 are drained together before the wait and the mutant survives this run —
    // the timing can only make the test weaker, never fail the correct code.)
    tokio::time::sleep(Duration::from_millis(300)).await;
    session
        .inject_message("c2".into(), Some(KIND.into()), false, None, true)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    gate.notify_one();

    // Wait until c2 is in the transcript — whichever turn carried it — then until everything idles.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let bodies = persisted_bodies(&session).await;
        if bodies.iter().any(|b| b.contains("c2")) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "c2 never persisted: {bodies:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    session.wait_for_idle().await;

    assert_eq!(
        faux.call_count(),
        2,
        "the user's turn plus ONE injected turn carrying both completions"
    );
    let bodies = persisted_bodies(&session).await;
    assert_eq!(bodies.len(), 1, "one merged completion message: {bodies:?}");
    assert!(
        bodies[0].contains("c1") && bodies[0].contains("c2"),
        "both completions ride the same message: {bodies:?}"
    );
}

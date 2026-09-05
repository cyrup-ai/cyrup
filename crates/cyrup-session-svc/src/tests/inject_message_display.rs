//! SUBA-094 — `display` must survive EVERY delivery arm of
//! [`crate::AgentSession::inject_message`], onto the live `MessageEnd` the TUI draws from AND into
//! the persisted entry `--resume` redraws from.
//!
//! The sibling of `inject_message_details`, and for the same structural reason: the three arms
//! reach persistence by three different routes, so a regression in one is invisible from the other
//! two. `display` was the field the trigger-turn route dropped — [`cyrup_agent::AgentMessage`]'s
//! `Custom` arm could not carry it, so the flag existed only as an argument to the DIRECT append
//! and every message that went through the run loop was persisted `display: true` and drawn.
//!
//! upstream: pi builds ONE `appMessage` carrying `display` and hands it to all five branches
//! (`coding-agent/src/core/agent-session.ts:1488-1517` @v0.84.4); the interactive host draws a
//! custom message only `if (message.display)` (`modes/interactive/interactive-mode.ts:3607-3620`).
//! `pi-subagents` is the producer that makes this observable: `notify.ts:402` @v0.64.0 sets
//! `display` from the completion outcome and `:408` passes it alongside `triggerTurn`, so a plain
//! successful background run is meant to reach the model without appearing on screen.
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
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use tempfile::TempDir;
use tokio_stream::StreamExt;

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig};

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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// The `display` of the one persisted custom message of `kind`, or `None` when the entry is absent.
async fn persisted_display(session: &Arc<crate::AgentSession>, kind: &str) -> Option<bool> {
    use cyrup_session::agent_message::AgentMessage as Raw;
    session
        .raw_context_messages()
        .await
        .into_iter()
        .find_map(|m| match m {
            Raw::Custom(c) if c.custom_type == kind => Some(c.display),
            _ => None,
        })
}

/// Wait for the persisted entry (the turn runs asynchronously) and return its `display`.
async fn await_persisted_display(session: &Arc<crate::AgentSession>, kind: &str) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(d) = persisted_display(session, kind).await {
            return d;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the injected message never persisted"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The `trigger_turn` arm (idle): the injected message becomes the INPUT of a fresh run, so it
/// reaches both surfaces through the run loop rather than through a direct append. This is the arm
/// `pi-subagents` completion notification uses (`trigger_turn: true`, `background/watch.rs`), and
/// the one that used to publish `display: true` unconditionally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_trigger_turn_arm_carries_display_false_through_the_run_loop() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();

    // Subscribe BEFORE injecting so the run's `MessageEnd` cannot be missed.
    let mut events = session.subscribe();

    session
        .inject_message(
            "Background run finished: docs-writer".to_string(),
            Some(KIND.to_string()),
            false,
            None,
            true,
        )
        .await
        .expect("the trigger-turn injection succeeds");

    // ---- the LIVE surface: the wire the TUI's serde projection reads its gate from. ----
    let mut seen: Option<serde_json::Value> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(ev)) = tokio::time::timeout_at(deadline, events.next()).await else {
            break;
        };
        if let AgentSessionEvent::MessageEnd { message } = ev {
            let v = serde_json::to_value(&message).unwrap();
            if v.get("kind").and_then(|t| t.as_str()) == Some(KIND) {
                seen = v.get("display").cloned();
                break;
            }
        }
    }
    assert_eq!(
        seen,
        Some(serde_json::Value::Bool(false)),
        "the live MessageEnd must carry display:false — this is the only thing the TUI can gate on"
    );

    // ---- the PERSISTED surface: `--resume` must not redraw what the live turn withheld. ----
    assert!(
        !await_persisted_display(&session, KIND).await,
        "the entry persisted through the run loop keeps display:false"
    );
}

/// The DURABLE arm (`trigger_turn: false`, idle): a direct append plus a `MessageStart`/`MessageEnd`
/// pair. The append already honoured `display`; the LIVE pair did not, because the message it
/// carried had no such field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_durable_arm_emits_display_false_on_message_end() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();

    let mut events = session.subscribe();

    session
        .inject_message(
            "Background run finished: docs-writer".to_string(),
            Some(KIND.to_string()),
            false,
            None,
            false,
        )
        .await
        .expect("the durable injection succeeds");

    let mut seen: Option<serde_json::Value> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(ev)) = tokio::time::timeout_at(deadline, events.next()).await else {
            break;
        };
        if let AgentSessionEvent::MessageEnd { message } = ev {
            let v = serde_json::to_value(&message).unwrap();
            if v.get("kind").and_then(|t| t.as_str()) == Some(KIND) {
                seen = v.get("display").cloned();
                break;
            }
        }
    }
    assert_eq!(
        seen,
        Some(serde_json::Value::Bool(false)),
        "the durable arm's live MessageEnd agrees with the entry it just appended"
    );
    assert!(
        !persisted_display(&session, KIND)
            .await
            .expect("the durable arm persisted a custom message"),
        "the durable arm's entry keeps display:false"
    );
}

/// A visible notice is still visible on the same arm — the gate is the flag, not the kind, and the
/// upstream predicate turns it on for every non-`completed` outcome (`notify.ts:402` @v0.64.0).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn display_true_still_rides_the_trigger_turn_arm() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();

    session
        .inject_message(
            "Background run FAILED: docs-writer".to_string(),
            Some(KIND.to_string()),
            true,
            None,
            true,
        )
        .await
        .expect("the trigger-turn injection succeeds");

    assert!(
        await_persisted_display(&session, KIND).await,
        "display:true survives the same route unchanged"
    );
}

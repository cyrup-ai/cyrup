//! ICOM-029 — `details` must survive EVERY delivery arm of
//! [`crate::AgentSession::inject_message`], both into the persisted session and onto the live
//! `MessageEnd` the TUI renders from.
//!
//! The seam has three arms and each reaches persistence by a different route, so a regression in one
//! is invisible from the other two:
//!
//! | arm | route to the persisted entry |
//! | --- | --- |
//! | streaming | `agent.steer(msg)` → the run loop → `subscriber.rs` `append_custom_message` |
//! | `trigger_turn` | `spawn_run(vec![msg])` → the run loop → the same append |
//! | durable | `append_custom_message(&kind, …, details)` called directly |
//!
//! `details` is what the intercom card renderer rebuilds its component from (ICOM-024): an entry
//! that persists without it draws the built-in `[intercom_message] body` framing instead of the
//! card, so "the message arrived" is NOT sufficient — the structured payload has to arrive with it.
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
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio_stream::StreamExt;

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig};

const KIND: &str = "intercom_message";

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

/// The card payload an inbound intercom message carries — the `InlineMessage` shape, kept here as
/// literal JSON so this crate does not take a dependency on `cyrup-intercom` just to describe it.
fn card() -> serde_json::Value {
    json!({
        "from": { "id": "peer-7", "label": "peer" },
        "message": { "id": "msg-42", "injectedAt": 1_700_000_000_123_u64 },
        "bodyText": "the body as the header rendered it",
        "collapsed": true
    })
}

/// The `details` of the one persisted custom message of `kind`, or `None` when the entry is absent.
async fn persisted_details(
    session: &std::sync::Arc<crate::AgentSession>,
    kind: &str,
) -> Option<serde_json::Value> {
    use cyrup_session::agent_message::AgentMessage as Raw;
    session
        .raw_context_messages()
        .await
        .into_iter()
        .find_map(|m| match m {
            Raw::Custom(c) if c.custom_type == kind => c.details,
            _ => None,
        })
}

/// The DURABLE arm (`trigger_turn: false`, idle): appends directly and surfaces via
/// `MessageStart`/`MessageEnd` without running a turn. Both halves must carry `details` — the
/// persisted entry is what `--resume` redraws from, the `MessageEnd` is what the live frame does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_durable_arm_persists_details_and_emits_them_on_message_end() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();

    // Subscribe BEFORE injecting so the emitted pair cannot be missed.
    let mut events = session.subscribe();

    session
        .inject_message(
            "body".to_string(),
            Some(KIND.to_string()),
            true,
            Some(card()),
            false,
        )
        .await
        .expect("the durable injection succeeds");

    // ---- the LIVE surface: `MessageEnd` carries the card (ICOM-024 renders from exactly this). ----
    let mut seen_end: Option<serde_json::Value> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(ev)) = tokio::time::timeout_at(deadline, events.next()).await else {
            break;
        };
        if let AgentSessionEvent::MessageEnd { message } = ev {
            let v = serde_json::to_value(&message).unwrap();
            // The AGENT wire form tags the type as `kind` (`event.rs` `TaggedNonAssistant`); the
            // PERSISTED form calls the same thing `customType` (`CustomRoleMessage`).
            if v.get("kind").and_then(|t| t.as_str()) == Some(KIND) {
                seen_end = v.get("details").cloned();
                break;
            }
        }
    }
    let end_details = seen_end.expect("a MessageEnd for the injected custom message was emitted");
    assert_eq!(
        end_details,
        card(),
        "the live MessageEnd carries the whole card, not a stripped twin"
    );

    // ---- the PERSISTED surface: the same object, byte-for-byte. ----
    let persisted = persisted_details(&session, KIND)
        .await
        .expect("the durable arm persisted a custom message carrying details");
    assert_eq!(
        persisted,
        card(),
        "the persisted entry carries the whole card"
    );
    assert_eq!(
        persisted, end_details,
        "live and persisted agree, so --resume redraws the same card"
    );

    // ---- the fields the renderer actually reads (ICOM-029 DoD 3/4). ----
    assert_eq!(persisted["from"]["id"], "peer-7");
    assert_eq!(persisted["message"]["id"], "msg-42");
    assert_eq!(
        persisted["message"]["injectedAt"], 1_700_000_000_123_u64,
        "the stamped injectedAt survives into the persisted details"
    );
    assert_eq!(
        persisted["bodyText"], "the body as the header rendered it",
        "bodyText survives as the string the card body was rendered from"
    );
}

/// The `trigger_turn` arm (idle): the injected message becomes the INPUT of a fresh run, so it
/// reaches persistence through the run loop's `MessageStart`/`MessageEnd` and the session
/// subscriber, not through a direct append.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_trigger_turn_arm_carries_details_through_the_run_loop() {
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
            "body".to_string(),
            Some(KIND.to_string()),
            true,
            Some(card()),
            true,
        )
        .await
        .expect("the trigger-turn injection succeeds");

    // The turn runs asynchronously; wait for the entry rather than racing it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let persisted = loop {
        if let Some(d) = persisted_details(&session, KIND).await {
            break d;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the injected message never persisted"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        persisted,
        card(),
        "spawn_run clones the whole AgentMessage into the run loop, so the card reaches the subscriber intact"
    );
}

/// The STEER arm: the session is mid-run, so the message joins the live turn's steering queue and
/// reaches persistence from inside that turn. The provider is held open on a `Notify` so the
/// injection lands while `is_streaming()` is genuinely true, rather than racing a turn that may
/// already have settled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_steer_arm_carries_details_through_the_live_turn() {
    let fx = fixture();
    let hold = Arc::new(Notify::new());
    let in_turn = Arc::new(Notify::new());

    let faux = Arc::new(FauxProvider::new());
    {
        let hold = hold.clone();
        let in_turn = in_turn.clone();
        faux.set_response_steps(vec![FauxResponseStep::async_factory(
            move |_c, _o, _s, _m| {
                let hold = hold.clone();
                let in_turn = in_turn.clone();
                async move {
                    // Announce that the turn is live, then park until the injection has landed.
                    in_turn.notify_one();
                    hold.notified().await;
                    faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
                }
            },
        )]);
    }

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();

    // Start a turn and wait until the provider is actually inside it.
    let runner = session.clone();
    tokio::spawn(async move {
        let _ = runner
            .send_user_message("drive a turn".to_string(), None)
            .await;
    });
    tokio::time::timeout(Duration::from_secs(10), in_turn.notified())
        .await
        .expect("the faux provider entered the turn");
    assert!(
        session.is_streaming().await,
        "the session is genuinely streaming before the injection"
    );

    session
        .inject_message(
            "body".to_string(),
            Some(KIND.to_string()),
            true,
            Some(card()),
            false,
        )
        .await
        .expect("the steered injection succeeds");

    // Let the held turn finish so the steered message flushes through the run loop.
    hold.notify_one();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let persisted = loop {
        if let Some(d) = persisted_details(&session, KIND).await {
            break d;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the steered message never persisted"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        persisted,
        card(),
        "a steered delivery reaches the subscriber with the card intact, exactly as the other two arms do"
    );
}

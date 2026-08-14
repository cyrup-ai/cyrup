//! SEAM-005 — `agent_settled`, the "the whole run is done" lifecycle event.
//!
//! Pi's `_emitAgentSettled()` (agent-session.ts:581-588) clears `_isAgentRunActive`, emits to the
//! EXTENSION RUNNER first and the session subscribers second, then resolves the idle wait. It is
//! called from the `finally` of `_runAgentPrompt` (:1063-1072) — i.e. AFTER the whole
//! `while (await this._handlePostAgentRun()) await this.agent.continue()` loop and after
//! `_flushPendingBashMessages()`. Its whole reason to exist is that `agent_end` CANNOT answer "is
//! more work coming?": an auto-retry, a post-run compaction, or an `agent_end`-queued continuation
//! each produce another `agent_end`. That is why Pi keys shutdown off `agent_settled` and nothing
//! else (rpc-mode.ts:355-358, interactive-mode.ts:3137).
//!
//! Before SEAM-005, `grep -rn "agent_settled" crates/` returned ZERO hits workspace-wide.
//!
//! The load-bearing assertion in every test here is the COUNT and the POSITION: it is trivial to
//! emit an event, and worthless to emit it at the wrong moment.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, FauxMessageOptions, FauxProvider,
};
use cyrup_provider::Provider;
use crate::{AgentSessionEvent, InputSource, SessionBuilder, SessionConfig, UserInput};
use futures::StreamExt;
use tempfile::TempDir;

/// A native built-in that counts the LIFECYCLE events it is dispatched, so a test can prove the
/// host actually invoked its `agent_settled` handler — not merely that the kind exists.
#[derive(Default)]
struct SettleCounter {
    settled: Arc<AtomicUsize>,
    ended: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for SettleCounter {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("settle-counter")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentEnd, EventKind::AgentSettled]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::AgentSettled => {
                self.settled.fetch_add(1, Ordering::SeqCst);
            }
            HostEvent::AgentEnd { .. } => {
                self.ended.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}

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
    cfg.no_extensions = true;
    cfg
}

fn fast_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("retry", serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 1}))
        .unwrap();
    cli
}

fn kinds(events: &[AgentSessionEvent]) -> Vec<&'static str> {
    events.iter().map(AgentSessionEvent::kind).collect()
}

/// THE headline proof, and the reason the event is not a synonym for `agent_end`: a turn that
/// AUTO-RETRIES produces TWO `agent_end`s and exactly ONE `agent_settled`, and the settle is LAST.
///
/// Pi's placement makes this precise — `_emitAgentSettled` is in `_runAgentPrompt`'s `finally`, so
/// it runs once the `while (await this._handlePostAgentRun())` loop has exhausted itself. A settle
/// emitted alongside `agent_end` would fire twice here and, worse, the FIRST one would claim the run
/// was over while a retry was already scheduled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_settled_fires_once_per_run_and_last_even_across_an_auto_retry() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Turn 1 = retryable transient error; turn 2 (the continuation) = clean success.
    faux.set_responses(vec![
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions { error_message: Some("overloaded".into()), ..Default::default() },
        ),
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);

    let counter = Arc::new(SettleCounter::default());
    let settled = counter.settled.clone();
    let ended = counter.ended.clone();

    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(fast_retry_settings())
        .with_native_extension(counter as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();

    let stream = session
        .prompt(UserInput::text("please do the thing", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The retry really happened (both scripted responses consumed).
    assert_eq!(faux.call_count(), 2, "the auto-retry continuation ran: {ks:?}");
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_end").count(),
        2,
        "two agent_end events — one per agent loop: {ks:?}"
    );

    // (1) The SUBSCRIBER stream saw exactly one settle, and it is the LAST event.
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_settled").count(),
        1,
        "exactly ONE agent_settled per run, however many agent_ends: {ks:?}"
    );
    assert_eq!(
        ks.last().copied(),
        Some("agent_settled"),
        "agent_settled is the final event of the run: {ks:?}"
    );

    // (2) The EXTENSION was dispatched it too — the host actually called the subscribed handler.
    assert_eq!(ended.load(Ordering::SeqCst), 2, "the extension saw both agent_end dispatches");
    assert_eq!(
        settled.load(Ordering::SeqCst),
        1,
        "the extension's agent_settled handler was invoked exactly once for the whole run"
    );
}

/// A plain single-turn run settles exactly once, after its one `agent_end`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_simple_run_settles_exactly_once_after_agent_end() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);

    let counter = Arc::new(SettleCounter::default());
    let settled = counter.settled.clone();

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(counter as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();

    let stream = session
        .prompt(UserInput::text("hello", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    let end_at = ks.iter().position(|k| *k == "agent_end").expect("agent_end: {ks:?}");
    let settle_at = ks.iter().position(|k| *k == "agent_settled").expect("agent_settled: {ks:?}");
    assert!(settle_at > end_at, "agent_settled follows agent_end: {ks:?}");
    assert_eq!(ks.iter().filter(|k| **k == "agent_settled").count(), 1, "{ks:?}");
    assert_eq!(settled.load(Ordering::SeqCst), 1, "the extension handler fired once");
}

/// Pi ALWAYS settles — `_emitAgentSettled` is in a `finally`. cyrup's UNBOUND (by-value) session has
/// no post-run driver at all: `spawn_run`'s `None` arm starts the run and returns, and the
/// persist+fan-out subscriber terminates the run-scoped streams on `agent_end`. Because no retry /
/// compaction / queued continuation can follow on that path, `agent_end` IS the settle point there,
/// and that is where the event is emitted — never earlier (emitting when `agent.prompt` returns
/// would fire while the model was still streaming).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unbound_session_settles_too() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);

    let counter = Arc::new(SettleCounter::default());
    let settled = counter.settled.clone();

    // NOT `into_shared()` — a by-value session, exactly the legacy path.
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(counter as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build");

    let stream = session
        .prompt(UserInput::text("hello", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    assert_eq!(
        ks.iter().filter(|k| **k == "agent_settled").count(),
        1,
        "an unbound session settles exactly once: {ks:?}"
    );
    assert_eq!(
        ks.last().copied(),
        Some("agent_settled"),
        "…and it is the last event before the run-scoped stream ends: {ks:?}"
    );
    assert_eq!(settled.load(Ordering::SeqCst), 1, "the extension handler fired on the unbound path");
}

/// The wire shape a front-end / RPC client sees (Pi `{ "type": "agent_settled" }`,
/// agent-session.ts:146).
#[test]
fn agent_settled_serializes_with_pis_wire_tag() {
    let wire = serde_json::to_value(AgentSessionEvent::AgentSettled).unwrap();
    assert_eq!(wire, serde_json::json!({ "type": "agent_settled" }));
}

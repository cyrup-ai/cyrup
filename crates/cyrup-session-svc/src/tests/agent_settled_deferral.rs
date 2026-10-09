//! SEAM-129 — a submission made while `agent_settled` is being delivered starts the next run.
//!
//! Pi `_emitAgentSettled` (`agent-session.ts:870-891` @v0.87.1) clears `_isAgentRunActive` BEFORE
//! it emits and sets `_isEmittingAgentSettled` around the emit. `prompt()` (`:1607-1609`) and
//! `sendMessage(…, {triggerTurn: true})` (`:1956-1958`) made during the emit are pushed onto
//! `_deferredSettledActions` and run right after it, before the idle wait resolves.
//!
//! cyrup emitted with its run latch still up and dropped it only afterwards, so the natural "run
//! the next task when the session settles" handler got `StreamingNeedsBehavior` for its prompt,
//! and a trigger-turn message was steered onto a run that was already over.
//!
//! Every test drives the real bound session: a native extension's `agent_settled` handler calls
//! back into the live `AgentSession` that is dispatching it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::{
    AgentSession, AgentSessionEvent, InputSource, SessionBuilder, SessionConfig,
    SessionServiceError, UserInput,
};
use cyrup_core::{Content, EventStream, ExtensionId, Message, StopReason};
use cyrup_ext::{
    ControlOp, EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use futures::StreamExt;
use serde_json::json;
use tempfile::TempDir;

/// What the handler submits from inside the first `agent_settled` it is dispatched.
#[derive(Clone, Copy)]
enum Submit {
    /// `session.prompt("NEXT")`.
    Prompt,
    /// `ctx.sendMessage({customType: "settled-note", …}, {triggerTurn: true})`.
    TriggerTurn,
    /// [`Self::Prompt`], then [`Self::TriggerTurn`] — two deferred actions, in that order.
    PromptThenTriggerTurn,
}

/// What the handler saw while the first `agent_settled` was being delivered.
#[derive(Default)]
struct Seen {
    run_active: Option<bool>,
    idle: Option<bool>,
    prompt: Option<Result<(), String>>,
    stream: Option<EventStream<AgentSessionEvent>>,
}

struct SettledSubmitter {
    submit: Submit,
    session: Arc<OnceLock<Weak<AgentSession>>>,
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    settled: Arc<AtomicUsize>,
    seen: Arc<Mutex<Seen>>,
}

#[async_trait::async_trait]
impl NativeExtension for SettledSubmitter {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("settled-submitter")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentSettled]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::AgentSettled { .. }) {
            return HookOutcome::Noop;
        }
        if self.settled.fetch_add(1, Ordering::SeqCst) != 0 {
            return HookOutcome::Noop;
        }
        let session = self.session.get().and_then(Weak::upgrade).unwrap();
        {
            let mut seen = self.seen.lock().unwrap();
            seen.run_active = Some(session.is_run_active());
            seen.idle = Some(session.is_idle());
        }
        if matches!(self.submit, Submit::Prompt | Submit::PromptThenTriggerTurn) {
            let result = session
                .prompt(UserInput::text("NEXT", InputSource::Sdk))
                .await;
            let mut seen = self.seen.lock().unwrap();
            match result {
                Ok(stream) => {
                    seen.prompt = Some(Ok(()));
                    seen.stream = Some(stream);
                }
                Err(e) => seen.prompt = Some(Err(e.to_string())),
            }
        }
        if matches!(
            self.submit,
            Submit::TriggerTurn | Submit::PromptThenTriggerTurn
        ) {
            let services = self.services.lock().unwrap().clone().unwrap();
            services
                .control(ControlOp::SendMessage {
                    message: json!({ "customType": "settled-note", "content": "go on" }),
                    opts: json!({ "triggerTurn": true }),
                })
                .unwrap();
            session.apply_pending_control().await;
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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

struct Harness {
    _fx: Fixture,
    faux: Arc<FauxProvider>,
    session: Arc<AgentSession>,
    settled: Arc<AtomicUsize>,
    seen: Arc<Mutex<Seen>>,
}

async fn harness(submit: Submit) -> Harness {
    harness_with(submit, None).await
}

/// [`harness`] with extra CLI-layer settings.
async fn harness_with(submit: Submit, cli_settings: Option<cyrup_config::Settings>) -> Harness {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ANSWER-ONE")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ANSWER-TWO")], StopReason::Stop),
    ]);
    let slot = Arc::new(OnceLock::new());
    let settled = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let ext = Arc::new(SettledSubmitter {
        submit,
        session: slot.clone(),
        services: Arc::new(Mutex::new(None)),
        settled: settled.clone(),
        seen: seen.clone(),
    });
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let mut builder = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_native_extension(ext as Arc<dyn NativeExtension>);
    if let Some(settings) = cli_settings {
        builder = builder.cli_settings(settings);
    }
    let session = builder.build().await.expect("build").into_shared();
    slot.set(Arc::downgrade(&session)).unwrap();
    Harness {
        _fx: fx,
        faux,
        session,
        settled,
        seen,
    }
}

fn assistant_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(
                a.content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

/// Run the first prompt and wait for the session to go idle, bounded so a regression that never
/// settles fails instead of hanging the suite.
async fn first_run_then_idle(h: &Harness) {
    let _stream = h
        .session
        .prompt(UserInput::text("FIRST", InputSource::Sdk))
        .await
        .expect("first prompt");
    tokio::time::timeout(Duration::from_secs(30), h.session.wait_for_idle())
        .await
        .expect("the session settles");
}

/// THE row: a prompt made from an `agent_settled` handler is accepted, starts the next run, and
/// `wait_for_idle` returns only after that run — not between the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_from_an_agent_settled_handler_starts_the_next_run() {
    let h = harness(Submit::Prompt).await;
    first_run_then_idle(&h).await;

    let prompt = h.seen.lock().unwrap().prompt.clone();
    assert_eq!(
        prompt,
        Some(Ok(())),
        "pi defers a prompt made during the agent_settled emit; cyrup refused it"
    );
    assert_eq!(
        h.faux.call_count(),
        2,
        "the deferred prompt ran a second turn"
    );
    assert_eq!(
        assistant_texts(&h.session.messages().await),
        vec!["ANSWER-ONE".to_string(), "ANSWER-TWO".to_string()],
        "wait_for_idle returned only after the deferred run had finished"
    );
    assert_eq!(h.settled.load(Ordering::SeqCst), 2, "each run settled once");
    assert!(h.session.is_idle());
}

/// The stream the deferred `prompt` returned belongs to the run it started: it carries that run's
/// answer and ends on that run's `agent_settled` — the settling run's `end_run` did not close it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deferred_prompts_stream_follows_the_run_it_started() {
    let h = harness(Submit::Prompt).await;
    first_run_then_idle(&h).await;

    let stream = h.seen.lock().unwrap().stream.take().expect("a stream");
    let events: Vec<AgentSessionEvent> =
        tokio::time::timeout(Duration::from_secs(10), stream.collect())
            .await
            .expect("the deferred run's stream ends");
    let kinds: Vec<&str> = events.iter().map(AgentSessionEvent::kind).collect();
    assert_eq!(kinds.last().copied(), Some("agent_settled"), "{kinds:?}");
    assert_eq!(
        kinds.iter().filter(|k| **k == "agent_settled").count(),
        1,
        "only the deferred run's settle, not the first run's: {kinds:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentSessionEvent::MessageEnd { message: cyrup_agent::AgentMessage::Assistant(a), .. }
                if a.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text.contains("ANSWER-TWO")))
        )),
        "the stream carries the deferred run's answer: {kinds:?}"
    );
}

/// Pi clears `_isAgentRunActive` before emitting, so a settled handler reads the session as not
/// streaming and idle (`isStreaming`/`isIdle`, `agent-session.ts:1229-1236` @v0.87.1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_settled_handler_sees_the_run_latch_released() {
    let h = harness(Submit::Prompt).await;
    first_run_then_idle(&h).await;

    let seen = h.seen.lock().unwrap();
    assert_eq!(
        seen.run_active,
        Some(false),
        "is_run_active inside agent_settled"
    );
    assert_eq!(seen.idle, Some(true), "is_idle inside agent_settled");
}

/// `sendMessage(…, {triggerTurn: true})` from the handler is deferred the same way and runs a turn
/// over the message; before, it was steered onto the finished run and no turn followed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trigger_turn_message_from_an_agent_settled_handler_runs_a_turn() {
    let h = harness(Submit::TriggerTurn).await;
    first_run_then_idle(&h).await;

    assert_eq!(
        h.faux.call_count(),
        2,
        "the deferred trigger-turn message ran a second turn"
    );
    assert_eq!(
        assistant_texts(&h.session.messages().await),
        vec!["ANSWER-ONE".to_string(), "ANSWER-TWO".to_string()],
    );
    let entries = h.session.entries_json().await;
    assert!(
        entries
            .iter()
            .any(|e| e.get("customType").and_then(|v| v.as_str()) == Some("settled-note")),
        "the custom message is on the transcript: {entries:?}"
    );
}

/// MIRROR: a prompt made while a run is genuinely still active is refused exactly as before — the
/// release window is the settle, not the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_during_the_run_itself_is_still_refused() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            tokens_per_second: Some(20.0),
            ..Default::default()
        },
    ));
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("a reply long enough to still be streaming")],
        StopReason::Stop,
    )]);
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build")
        .into_shared();
    let _stream = session
        .prompt(UserInput::text("FIRST", InputSource::Sdk))
        .await
        .expect("first prompt");
    let err = session
        .prompt(UserInput::text("SECOND", InputSource::Sdk))
        .await
        .err()
        .expect("a second prompt mid-run is refused");
    assert!(
        matches!(err, SessionServiceError::StreamingNeedsBehavior),
        "{err:?}"
    );
    session.abort_and_settle().await;
}

/// Pi runs the deferred actions as `for (const action of deferred) await action();`
/// (`agent-session.ts:883` @v0.87.1), so the first one that throws ends the loop and the actions
/// after it never run. Here the deferred prompt throws from its pre-prompt compaction check (an
/// invalid `compaction.keepRecentTokens`, `getCompactionSettings` at `:1697`), and the trigger-turn
/// message deferred after it must not start a turn of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_deferred_action_drops_the_actions_after_it() {
    let mut settings = cyrup_config::Settings::new();
    settings
        .set_field(
            "compaction",
            json!({ "enabled": false, "keepRecentTokens": 2.5 }),
        )
        .unwrap();
    let h = harness_with(Submit::PromptThenTriggerTurn, Some(settings)).await;
    first_run_then_idle(&h).await;

    assert_eq!(
        h.seen.lock().unwrap().prompt,
        Some(Ok(())),
        "the prompt itself was deferred, not refused"
    );
    assert_eq!(
        h.faux.call_count(),
        1,
        "the deferred prompt threw, so the trigger-turn message queued behind it never ran"
    );
    let stream = h.seen.lock().unwrap().stream.take().expect("a stream");
    let events: Vec<AgentSessionEvent> =
        tokio::time::timeout(Duration::from_secs(10), stream.collect())
            .await
            .expect("the failed prompt's stream ends");
    assert!(
        events.is_empty(),
        "no run was started for it: {:?}",
        events
            .iter()
            .map(AgentSessionEvent::kind)
            .collect::<Vec<_>>()
    );
    assert!(h.session.is_idle());
}

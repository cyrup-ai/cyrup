//! `HostServices::send_user_message` — pi `sendUserMessage(text, { deliverAs })` from a native
//! extension's own task runs the REAL prompt path, not the injected-turn bypass.
//!
//! The subagent steering inbox (`cyrup-ext-subagents/src/prompt_runtime.rs`) and the watchdog's
//! auto-follow both send a USER message from a timer / boundary hook. They used to go through
//! `inject_message(…, custom_type: None, trigger_turn: true)`, whose turn `run_injection` hands
//! straight to the agent at the next idle edge: `input` and `before_agent_start` never ran (pi's own
//! `_runAgentPrompt` bypass, pi#5581 — the defect ICOM-084 fixed for intercom), and a busy child was
//! not steered mid-run at all. pi's `sendUserMessage` is `prompt(text, { streamingBehavior:
//! deliverAs, source: "extension" })` (`agent-session.ts:2365-2389` @v1.1.0): `input` always runs;
//! idle → a full run; streaming → queued onto that run.
//!
//! Killing mutations: route `InjectItem::UserMessage` through `run_injection` (the
//! `before_agent_start` / `input` counts drop); skip the busy-phase `run_user_messages` (the steer
//! waits for the idle edge, so its `input` never fires while the run is held and the second test
//! times out); steer a no-delivery message onto the live run, or drop it, instead of holding it for
//! the idle edge (the third test sees one `before_agent_start`, not two).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{AgentSession, InputSource, SessionBuilder, SessionConfig, UserInput};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    InputStreamingBehavior, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use tempfile::TempDir;

const GUIDANCE: &str = "Mid-run steering from the parent orchestrator: focus on the test.";

/// Records every `input` (text + the streaming selector it was handed) and `before_agent_start`
/// prompt, counts `agent_start`, and hands its host services to the test — the position the
/// steering inbox's poll task is in.
#[derive(Default)]
struct LifecycleProbe {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    input: Mutex<Vec<(String, Option<InputStreamingBehavior>)>>,
    before_agent_start: Mutex<Vec<String>>,
    agent_start: AtomicUsize,
}

#[async_trait::async_trait]
impl NativeExtension for LifecycleProbe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("send-user-message-probe")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[
            EventKind::Input,
            EventKind::BeforeAgentStart,
            EventKind::AgentStart,
        ]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input {
                text,
                streaming_behavior,
                ..
            } => {
                self.input
                    .lock()
                    .unwrap()
                    .push((text.clone(), *streaming_behavior));
            }
            HostEvent::BeforeAgentStart { prompt, .. } => {
                self.before_agent_start.lock().unwrap().push(prompt.clone());
            }
            HostEvent::AgentStart => {
                self.agent_start.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}

impl LifecycleProbe {
    fn services(&self) -> Arc<dyn HostServices> {
        self.services
            .lock()
            .unwrap()
            .clone()
            .expect("host services attached")
    }

    fn prompts(&self) -> Vec<String> {
        self.before_agent_start.lock().unwrap().clone()
    }

    fn inputs(&self) -> Vec<(String, Option<InputStreamingBehavior>)> {
        self.input.lock().unwrap().clone()
    }
}

struct Fixture {
    _tmp: TempDir,
    session: Arc<AgentSession>,
    probe: Arc<LifecycleProbe>,
}

async fn fixture(faux: &Arc<FauxProvider>) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let probe = Arc::new(LifecycleProbe::default());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_native_extension(probe.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    Fixture {
        _tmp: tmp,
        session,
        probe,
    }
}

/// A provider step that records the request's messages (debug-rendered) and answers at once.
fn recording_step(seen: &Arc<Mutex<Vec<String>>>, reply: &'static str) -> FauxResponseStep {
    let seen = seen.clone();
    FauxResponseStep::factory(move |ctx, _, _, _| {
        seen.lock().unwrap().push(format!("{:?}", ctx.messages));
        faux_assistant_message(vec![faux_text(reply)], StopReason::Stop)
    })
}

/// A first provider step that holds the run open until `gate` is notified.
fn held_step(gate: &Arc<tokio::sync::Notify>) -> FauxResponseStep {
    let held = gate.clone();
    FauxResponseStep::async_factory(move |_, _, _, _| {
        let held = held.clone();
        async move {
            held.notified().await;
            faux_assistant_message(vec![faux_text("human turn")], StopReason::Stop)
        }
    })
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn settle(session: &AgentSession) {
    tokio::time::timeout(Duration::from_secs(15), session.wait_for_idle())
        .await
        .expect("settles");
    // A window for a further run to start if one were wrongly owed. Only NEGATIVE checks lean on
    // it: a slow scheduler can make them pass vacuously, never fail spuriously.
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.wait_for_idle().await;
}

/// Idle child: the steer starts a run through the WHOLE lifecycle — `input` (with no streaming
/// selector, pi passes `undefined` when idle), `before_agent_start` over the text, `agent_start` —
/// and the model sees it as the user turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_session_runs_the_message_through_the_prompt_lifecycle() {
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    faux.set_response_steps(vec![recording_step(&seen, "on it")]);
    let fx = fixture(&faux).await;

    fx.probe
        .services()
        .send_user_message(GUIDANCE, Some(InputStreamingBehavior::Steer))
        .unwrap();
    wait_until("one model call", || faux.call_count() >= 1).await;
    settle(&fx.session).await;

    assert_eq!(fx.probe.inputs(), vec![(GUIDANCE.to_string(), None)]);
    assert_eq!(fx.probe.prompts(), vec![GUIDANCE.to_string()]);
    assert_eq!(fx.probe.agent_start.load(Ordering::SeqCst), 1);
    assert_eq!(faux.call_count(), 1);
    assert!(seen.lock().unwrap()[0].contains(GUIDANCE));
}

/// Busy child: a `deliverAs: "steer"` message is STEERED onto the live run (pi `_queueSteer`),
/// after `input` saw it with the `steer` selector. The held run's next request carries it, and no
/// second run starts: two model calls, one `before_agent_start` (the human's), one `agent_start`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_session_is_steered_mid_run() {
    let faux = Arc::new(FauxProvider::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    faux.set_response_steps(vec![
        held_step(&gate),
        recording_step(&seen, "incorporated the guidance"),
        recording_step(&seen, "a run nobody asked for"),
    ]);
    let fx = fixture(&faux).await;

    let _stream = fx
        .session
        .prompt(UserInput::text("human task", InputSource::Sdk))
        .await
        .expect("prompt");
    wait_until("the held call", || faux.call_count() >= 1).await;

    fx.probe
        .services()
        .send_user_message(GUIDANCE, Some(InputStreamingBehavior::Steer))
        .unwrap();
    // Release the run only once the pump has handed the steer to the prompt path — its `input`
    // dispatch is the observable edge — so a late pump cannot make the run finish first.
    let probe = fx.probe.clone();
    wait_until("the steer's input event", move || probe.inputs().len() >= 2).await;
    gate.notify_one();
    settle(&fx.session).await;

    assert_eq!(
        fx.probe.inputs(),
        vec![
            ("human task".to_string(), None),
            (GUIDANCE.to_string(), Some(InputStreamingBehavior::Steer)),
        ]
    );
    assert_eq!(faux.call_count(), 2, "the steer continued the live run");
    assert!(
        seen.lock().unwrap()[0].contains(GUIDANCE),
        "the run's next request carries the steer"
    );
    assert_eq!(fx.probe.prompts(), vec!["human task".to_string()]);
    assert_eq!(fx.probe.agent_start.load(Ordering::SeqCst), 1);
}

/// Busy child, NO delivery named (the watchdog's bare `sendUserMessage(message)`): pi would throw
/// into a `void`-ed promise. The pump instead holds the message and runs it as its own prompt at the
/// idle edge — a second run that, unlike the old injected turn, runs `before_agent_start`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_without_a_delivery_waits_for_the_idle_edge() {
    let faux = Arc::new(FauxProvider::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    faux.set_response_steps(vec![
        held_step(&gate),
        recording_step(&seen, "addressed the blocker"),
    ]);
    let fx = fixture(&faux).await;

    let _stream = fx
        .session
        .prompt(UserInput::text("human task", InputSource::Sdk))
        .await
        .expect("prompt");
    wait_until("the held call", || faux.call_count() >= 1).await;

    fx.probe
        .services()
        .send_user_message("Watchdog auto-follow: fix it.", None)
        .unwrap();
    // Nothing to observe while held (no `input` runs until the idle edge), so give the pump a
    // moment to take it against the busy run before releasing — the assertion below holds whether
    // or not it got there first, since an idle-edge prompt is the outcome either way.
    tokio::time::sleep(Duration::from_millis(100)).await;
    gate.notify_one();
    wait_until("the held message's own run", || faux.call_count() >= 2).await;
    settle(&fx.session).await;

    assert_eq!(faux.call_count(), 2);
    assert_eq!(
        fx.probe.prompts(),
        vec![
            "human task".to_string(),
            "Watchdog auto-follow: fix it.".to_string()
        ],
        "the held message ran as a real prompt of its own"
    );
    assert_eq!(fx.probe.agent_start.load(Ordering::SeqCst), 2);
    assert!(seen.lock().unwrap()[0].contains("Watchdog auto-follow: fix it."));
}

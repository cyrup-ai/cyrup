//! ICOM-084 — `HostServices::wake_user_prompt`: an idle session woken by a native extension's
//! background task runs the NORMAL prompt lifecycle, after the card it announces.
//!
//! pi-intercom `104b83c` (#154, v0.16.1) stopped waking an idle session with
//! `sendMessage(card, { triggerTurn: true })`, because pi runs that turn through
//! `_runAgentPrompt` and skips `before_agent_start` (pi#5581) — cyrup's `run_injection` is the same
//! bypass. It now hands the card over with no turn and wakes the session with
//! `sendUserMessage("New intercom message above.")`, which is `prompt()` and runs every hook. These
//! tests pin the host half: the wake reaches `before_agent_start` and `agent_start`, runs strictly
//! after the card (FIFO through the injection pump), coalesces, and never starts a second run on top
//! of one that already carries the card.
//!
//! Killing mutations: route `InjectItem::WakePrompt` through `run_injection` instead of
//! `prompt_with` (the `before_agent_start` count drops to 0); run the wake before the durable append
//! (the card follows the wake in the request); drop the `wakes.clear()` on an active run (a third
//! model call).
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
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use tempfile::TempDir;

const WAKE: &str = "New intercom message above.";
const CARD_KIND: &str = "intercom_message";

/// Records every `before_agent_start` prompt and counts `agent_start`, and hands its host services
/// to the test (the background-task position pi-intercom's broker handler is in).
#[derive(Default)]
struct LifecycleProbe {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    before_agent_start: Mutex<Vec<String>>,
    agent_start: AtomicUsize,
}

#[async_trait::async_trait]
impl NativeExtension for LifecycleProbe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("wake-lifecycle-probe")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart, EventKind::AgentStart]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
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

async fn wait_for_calls(faux: &FauxProvider, n: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while faux.call_count() < n {
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {n} model call(s), saw {}",
            faux.call_count()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn position(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in the request: {haystack}"))
}

/// pi-intercom `human-priority.pi.test.ts` "idle trigger wakes through the prompt lifecycle"
/// (`104b83c`), host half: the card goes in with no turn, the wake prompt starts the run, and that
/// run is a real prompt — `before_agent_start` sees the wake text and `agent_start` fires — whose
/// request carries the card BEFORE the wake.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_wake_runs_the_prompt_lifecycle_after_the_card() {
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    faux.set_response_steps(vec![recording_step(&seen, "answered the peer")]);
    let fx = fixture(&faux).await;
    let services = fx.probe.services();

    services
        .inject_message_steer("peer card body", Some(CARD_KIND), true, None)
        .unwrap();
    services.wake_user_prompt(WAKE).unwrap();

    wait_for_calls(&faux, 1).await;
    tokio::time::timeout(Duration::from_secs(15), fx.session.wait_for_idle())
        .await
        .expect("the woken run settles");

    assert_eq!(
        fx.probe.prompts(),
        vec![WAKE.to_string()],
        "the woken turn ran before_agent_start exactly once, over the wake prompt"
    );
    assert_eq!(fx.probe.agent_start.load(Ordering::SeqCst), 1);
    let request = seen.lock().unwrap()[0].clone();
    assert!(
        position(&request, "peer card body") < position(&request, WAKE),
        "the card precedes the wake in the model's request: {request}"
    );

    // Transcript order: [custom intercom card, user wake, assistant].
    let kinds: Vec<&'static str> = fx
        .session
        .agent_messages()
        .await
        .iter()
        // The prompt's own system record (written at a session's first prompt) is not under test.
        .filter(|m| !matches!(m, cyrup_agent::AgentMessage::System(_)))
        .map(|m| match m {
            cyrup_agent::AgentMessage::Custom { kind, .. } if kind == CARD_KIND => "card",
            cyrup_agent::AgentMessage::User { .. } => "user",
            cyrup_agent::AgentMessage::Assistant(_) => "assistant",
            other => panic!("unexpected transcript message: {other:?}"),
        })
        .collect();
    assert_eq!(kinds, vec!["card", "user", "assistant"]);
    assert_eq!(faux.call_count(), 1);
}

/// Several cards and wakes handed over back to back are ONE woken run that carries every card —
/// the wakes are fire-and-forget nudges, and a second run would answer nothing new.
///
/// Single-threaded ON PURPOSE: the four hand-overs are synchronous and the test task does not
/// yield between them, so the pump cannot run until all four are queued and its first drain takes
/// them together. On a multi-threaded runtime the pump could settle the first card and wake, run,
/// and go idle before the second card is queued — a legitimate second run, and a flaky assertion.
#[tokio::test(flavor = "current_thread")]
async fn wakes_settled_together_coalesce_into_one_run() {
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    faux.set_response_steps(vec![
        recording_step(&seen, "answered both"),
        recording_step(&seen, "a run nobody asked for"),
    ]);
    let fx = fixture(&faux).await;
    let services = fx.probe.services();

    services
        .inject_message_steer("first card", Some(CARD_KIND), true, None)
        .unwrap();
    services.wake_user_prompt(WAKE).unwrap();
    services
        .inject_message_steer("second card", Some(CARD_KIND), true, None)
        .unwrap();
    services.wake_user_prompt(WAKE).unwrap();

    wait_for_calls(&faux, 1).await;
    tokio::time::timeout(Duration::from_secs(15), fx.session.wait_for_idle())
        .await
        .expect("settles");
    // A window for a second run to start if the pump wrongly owed one. Only a NEGATIVE check
    // waits here: a slow scheduler can make it pass vacuously, never fail spuriously.
    tokio::time::sleep(Duration::from_millis(200)).await;
    fx.session.wait_for_idle().await;

    let request = seen.lock().unwrap()[0].clone();
    assert!(position(&request, "first card") < position(&request, "second card"));
    assert!(position(&request, "second card") < position(&request, WAKE));
    assert_eq!(faux.call_count(), 1, "one woken run");
    assert_eq!(fx.probe.prompts(), vec![WAKE.to_string()]);
    assert_eq!(
        fx.session
            .wakes_dropped_on_active_run
            .load(Ordering::SeqCst),
        0,
        "no run held the session, so no wake was dropped: they coalesced"
    );
}

/// pi-intercom wakes only an IDLE session (`index.ts:1300` @v0.16.1). A wake that reaches the pump
/// while a run holds the session is dropped: the card was steered onto that run, which answers it,
/// so waking afterwards would be a second run — and pi's own `sendUserMessage` on a streaming
/// session without `deliverAs` is refused (`agent-session.ts:1655-1659`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_that_meets_an_active_run_starts_no_second_run() {
    let faux = Arc::new(FauxProvider::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let held = gate.clone();
    faux.set_response_steps(vec![
        FauxResponseStep::async_factory(move |_, _, _, _| {
            let held = held.clone();
            async move {
                held.notified().await;
                faux_assistant_message(vec![faux_text("human turn")], StopReason::Stop)
            }
        }),
        faux_assistant_message(vec![faux_text("saw the steered card")], StopReason::Stop).into(),
        faux_assistant_message(vec![faux_text("a run nobody asked for")], StopReason::Stop).into(),
    ]);
    let fx = fixture(&faux).await;
    let services = fx.probe.services();

    let _stream = fx
        .session
        .prompt(UserInput::text("human task", InputSource::Sdk))
        .await
        .expect("prompt");
    wait_for_calls(&faux, 1).await;

    services
        .inject_message_steer("peer card body", Some(CARD_KIND), true, None)
        .unwrap();
    services.wake_user_prompt(WAKE).unwrap();
    // Release the run only once the pump has taken the wake and dropped it against the held run —
    // a condition, not a sleep, so a late-scheduled pump cannot make the run finish first. The card
    // was queued before the wake (FIFO), so it has been taken too.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while fx
        .session
        .wakes_dropped_on_active_run
        .load(Ordering::SeqCst)
        == 0
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pump never saw the wake while the run was held"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    gate.notify_one();

    tokio::time::timeout(Duration::from_secs(15), fx.session.wait_for_idle())
        .await
        .expect("settles");
    // A window for a third call to start if a wake survived. Only a NEGATIVE check waits here: a
    // slow scheduler can make it pass vacuously, never fail spuriously.
    tokio::time::sleep(Duration::from_millis(300)).await;
    fx.session.wait_for_idle().await;

    assert_eq!(
        faux.call_count(),
        2,
        "the steered card continued the human's run; the wake started nothing"
    );
    assert_eq!(fx.probe.prompts(), vec!["human task".to_string()]);
    assert_eq!(fx.probe.agent_start.load(Ordering::SeqCst), 1);
    assert_eq!(
        fx.session
            .wakes_dropped_on_active_run
            .load(Ordering::SeqCst),
        1,
        "exactly the one wake was dropped against the held run"
    );
}

/// The wake text is sent AS TEXT (`expandPromptTemplates: false`): a wake that starts with `/`
/// reaches the model rather than dispatching a command or expanding a template.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_prompt_is_never_expanded_as_a_command() {
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    faux.set_response_steps(vec![recording_step(&seen, "ok")]);
    let fx = fixture(&faux).await;

    fx.probe
        .services()
        .wake_user_prompt("/compact now")
        .unwrap();
    wait_for_calls(&faux, 1).await;
    tokio::time::timeout(Duration::from_secs(15), fx.session.wait_for_idle())
        .await
        .expect("settles");
    assert_eq!(fx.probe.prompts(), vec!["/compact now".to_string()]);
    assert!(seen.lock().unwrap()[0].contains("/compact now"));
}

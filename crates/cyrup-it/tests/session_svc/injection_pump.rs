//! The session's injection pump and idle latches, driven over a REAL `AgentSession` (built by
//! `SessionBuilder`, bound with `into_shared`, so the pump and the post-run driver are live) and a
//! scripted faux provider whose requests are captured. Every assertion is about what the MODEL was
//! sent, or about the session's own latches — never about a `HostServices` double.
//!
//! * **ICOM-035** — a `deliverAs: "steer"` injection (`HostServices::inject_message_steer`, pi-intercom's
//!   busy delivery) that arrives while a run is active is steered onto THAT run (pi `agent.steer`,
//!   `agent-session.ts:1949-1954` @v0.87.1): it is in the run's next request, and no extra turn
//!   follows; a PLAIN no-turn injection is not steered and never extends a run. And the race the
//!   pump was built for (`8de7460`): a steer that lands after the run's last steering poll is not
//!   stranded in the agent queue — the idle edge takes it back and appends it, exactly once.
//! * **SEAM-129** — `agent_settled` is emitted with the run latch already released (pi
//!   `_emitAgentSettled`, `agent-session.ts:870-873` @v0.87.1), so a steer sent from its handler is
//!   appended during the emit rather than steered onto the finished run.
//! * **ICOM-068** — a no-turn injection while idle, and `send_custom_message`'s idle arm, reach the
//!   agent transcript as well as the tree (pi `_appendCustomMessage` → `_refreshFinalizedContext`,
//!   `:1968-1982`, `:730-736`), so the next prompt's request carries them, before the prompt.
//! * **SEAM-125** — a running manual compaction counts as busy: `is_idle()` is false (while
//!   `is_run_active()` stays false, pi's `isStreaming`), `wait_for_idle()` resolves only after
//!   `compaction_end`, and an injected turn does not start a model run under the compaction.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use cyrup_core::{Content, ExtensionId, Message, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::{Context, Provider, StreamOptions};
use cyrup_session_svc::{AgentSession, InputSource, SessionBuilder, SessionConfig, UserInput};
use tempfile::TempDir;

const NOTE: &str = "note";

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
    std::fs::write(cwd.join("notes.txt"), "file contents\n").unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// Compaction that works on a two-turn session (keep nothing, reserve nothing), with retries off so
/// a summarizer failure is one provider call.
fn compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli.set_field("retry", serde_json::json!({"enabled": false}))
        .unwrap();
    cli
}

/// Every provider request's messages, in call order.
type Requests = Arc<Mutex<Vec<Vec<Message>>>>;

/// The text of every USER-role message in a request, in order — a custom message reaches the
/// provider as a user message carrying its content (pi `convertToLlm`'s `case "custom"`).
fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
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

fn position_of(texts: &[String], needle: &str) -> Option<usize> {
    texts.iter().position(|t| t.contains(needle))
}

fn count_of(texts: &[String], needle: &str) -> usize {
    texts.iter().filter(|t| t.contains(needle)).count()
}

/// A step that records its request and answers `reply` at once.
fn recording_text(requests: &Requests, reply: &'static str) -> FauxResponseStep {
    let requests = requests.clone();
    FauxResponseStep::factory(move |ctx: &Context, _, _, _| {
        requests.lock().unwrap().push(ctx.messages.clone());
        faux_assistant_message(vec![faux_text(reply)], StopReason::Stop)
    })
}

/// A step that records its request and answers with a `read` tool call.
fn recording_tool_call(requests: &Requests) -> FauxResponseStep {
    let requests = requests.clone();
    FauxResponseStep::factory(move |ctx: &Context, _, _, _| {
        requests.lock().unwrap().push(ctx.messages.clone());
        faux_assistant_message(
            vec![faux_tool_call(
                "read",
                serde_json::json!({ "path": "notes.txt" }),
            )],
            StopReason::ToolUse,
        )
    })
}

/// Poll `predicate` until it holds or `budget` elapses.
async fn within(budget: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Every persisted custom-message body of `kind`, in tree order.
async fn persisted(session: &Arc<AgentSession>, kind: &str) -> Vec<String> {
    use cyrup_session::agent_message::AgentMessage as Raw;
    session
        .raw_context_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            Raw::Custom(c) if c.custom_type == kind => Some(c.content.to_string()),
            _ => None,
        })
        .collect()
}

async fn wait_persisted(session: &Arc<AgentSession>, kind: &str, n: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let bodies = persisted(session, kind).await;
        if bodies.len() >= n || tokio::time::Instant::now() >= deadline {
            return bodies;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn build(
    fx: &Fixture,
    faux: &Arc<FauxProvider>,
    ext: Option<Arc<dyn NativeExtension>>,
) -> Arc<AgentSession> {
    let mut builder = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, base_config(fx))
        .cli_settings(compaction_settings());
    if let Some(ext) = ext {
        builder = builder.with_native_extension(ext);
    }
    let session = builder.build().await.unwrap().into_shared();
    session.bind_extensions().await;
    session
}

/// ICOM-068 — pi's idle no-trigger arm is `_appendCustomMessage`: tree AND transcript. The pump's
/// durable arm used to write only the tree, so this message was drawn and persisted and then
/// missing from the very next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_no_turn_injection_while_idle_reaches_the_next_prompt() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![recording_text(&requests, "ok")]);
    let session = build(&fx, &faux, None).await;

    session
        .inject_message(
            "peer says: use the v2 schema".into(),
            Some(NOTE.into()),
            true,
            None,
            false,
        )
        .await
        .unwrap();
    let bodies = wait_persisted(&session, NOTE, 1).await;
    assert_eq!(
        bodies.len(),
        1,
        "the no-turn injection is in the tree: {bodies:?}"
    );
    assert_eq!(faux.call_count(), 0, "a no-turn injection starts no run");

    let _ = session
        .prompt(UserInput::text("next", InputSource::Sdk))
        .await
        .unwrap();
    session.wait_for_idle().await;

    let requests = requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let texts = user_texts(&requests[0]);
    let note = position_of(&texts, "use the v2 schema")
        .unwrap_or_else(|| panic!("the injected message reached the model: {texts:?}"));
    let next = position_of(&texts, "next").unwrap();
    assert!(
        note < next,
        "the injected message precedes the prompt: {texts:?}"
    );
    assert_eq!(
        count_of(&texts, "use the v2 schema"),
        1,
        "exactly once: {texts:?}"
    );
}

/// ICOM-068 — `send_custom_message`'s idle arm (no `deliver_as`) had the same tree-only append.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_custom_message_while_idle_reaches_the_next_prompt() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![recording_text(&requests, "ok")]);
    let session = build(&fx, &faux, None).await;

    session
        .send_custom_message(
            NOTE,
            serde_json::json!("remember the staging freeze"),
            true,
            None,
            None,
            // SEAM-127 added pi's raw `options?.triggerTurn`: absent here, which is pi's own
            // `sendCustomMessage(msg)` — the idle no-trigger arm this test is about.
            None,
        )
        .await
        .unwrap();
    assert_eq!(persisted(&session, NOTE).await.len(), 1);

    let _ = session
        .prompt(UserInput::text("next", InputSource::Sdk))
        .await
        .unwrap();
    session.wait_for_idle().await;
    let requests = requests.lock().unwrap().clone();
    let texts = user_texts(&requests[0]);
    let note = position_of(&texts, "staging freeze")
        .unwrap_or_else(|| panic!("the custom message reached the model: {texts:?}"));
    assert!(note < position_of(&texts, "next").unwrap(), "{texts:?}");
}

/// A native extension that only captures the session's `HostServices` — the backend every native
/// built-in (pi-intercom included) injects through. (A [`SettleInjector`] that has already fired.)
fn probe() -> Arc<SettleInjector> {
    Arc::new(SettleInjector {
        services: Mutex::new(None),
        session: Arc::new(OnceLock::new()),
        fired: AtomicBool::new(true),
        appended_during_emit: AtomicBool::new(false),
    })
}

/// ICOM-035 at the session seam — a `deliverAs: "steer"` injection
/// (`HostServices::inject_message_steer`, pi-intercom's busy delivery) that arrives while a run is
/// in flight is steered into THAT run: the run's second request (after its first tool call)
/// carries it, it is persisted once, and no extra turn follows. Before the fix the pump parked it
/// until the run was over and then appended it with no turn, so the second request did not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_injection_during_a_run_is_steered_into_that_run() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let faux = Arc::new(FauxProvider::new());
    let steered = Arc::new(OnceLock::<Weak<AgentSession>>::new());
    let first = {
        let requests = requests.clone();
        let steered = steered.clone();
        // Request 1 is held open until the injection has been handed to the run's steering queue,
        // so the steer deterministically lands before the run's first steering poll.
        FauxResponseStep::async_factory(move |ctx: Context, _, _, _| {
            let requests = requests.clone();
            let steered = steered.clone();
            async move {
                requests.lock().unwrap().push(ctx.messages.clone());
                let queued = within(Duration::from_secs(10), || {
                    steered
                        .get()
                        .and_then(Weak::upgrade)
                        .is_some_and(|s| s.has_queued_messages())
                })
                .await;
                assert!(queued, "the injection was never steered onto the live run");
                faux_assistant_message(
                    vec![faux_tool_call(
                        "read",
                        serde_json::json!({ "path": "notes.txt" }),
                    )],
                    StopReason::ToolUse,
                )
            }
        })
    };
    faux.set_response_steps(vec![
        first,
        recording_tool_call(&requests),
        recording_text(&requests, "done"),
    ]);
    let probe = probe();
    let session = build(&fx, &faux, Some(probe.clone())).await;
    let _ = steered.set(Arc::downgrade(&session));
    let services = probe.services.lock().unwrap().clone().unwrap();

    let _ = session
        .prompt(UserInput::text("start the task", InputSource::Sdk))
        .await
        .unwrap();
    assert!(within(Duration::from_secs(10), || faux.call_count() >= 1).await);
    assert!(session.is_run_active());
    services
        .inject_message_steer(
            "supervisor: stop and use the v2 schema",
            Some(NOTE),
            true,
            None,
        )
        .unwrap();
    session.wait_for_idle().await;

    let requests = requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        3,
        "one run of three requests, no extra turn"
    );
    assert!(
        !user_texts(&requests[0])
            .iter()
            .any(|t| t.contains("v2 schema")),
        "request 1 predates the message"
    );
    let second = user_texts(&requests[1]);
    assert_eq!(
        count_of(&second, "v2 schema"),
        1,
        "the steered message is in the SAME run's second request: {second:?}"
    );
    assert_eq!(
        persisted(&session, NOTE).await.len(),
        1,
        "persisted exactly once"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(faux.call_count(), 3, "no turn was started after the run");
    assert!(!session.has_queued_messages());
}

/// The other half of ICOM-035's distinction — a PLAIN no-turn injection (`inject_message(…, false)`,
/// pi `{ triggerTurn: false }`) is NOT steered: pi defers it on a streaming session
/// (`_pendingCustomMessages`, `agent-session.ts:1962-1967` @v0.87.1) because a steer the model
/// answers can extend the run. Arriving during a run's final request, it must not buy a
/// continuation: the run ends after one request, the message is then appended (tree + transcript,
/// ICOM-068), and it reaches the model on the next prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_no_turn_injection_during_a_run_does_not_extend_it() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let faux = Arc::new(FauxProvider::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let held = {
        let requests = requests.clone();
        let gate = gate.clone();
        FauxResponseStep::async_factory(move |ctx: Context, _, _, _| {
            let requests = requests.clone();
            let gate = gate.clone();
            async move {
                requests.lock().unwrap().push(ctx.messages.clone());
                gate.notified().await;
                faux_assistant_message(vec![faux_text("final answer")], StopReason::Stop)
            }
        })
    };
    faux.set_response_steps(vec![held, recording_text(&requests, "ok")]);
    let session = build(&fx, &faux, None).await;

    let _ = session
        .prompt(UserInput::text("start", InputSource::Sdk))
        .await
        .unwrap();
    assert!(within(Duration::from_secs(10), || faux.call_count() == 1).await);
    session
        .inject_message(
            "background notice".into(),
            Some(NOTE.into()),
            true,
            None,
            false,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !session.has_queued_messages(),
        "a plain no-turn message is not handed to the run's steering queue"
    );
    gate.notify_one();
    session.wait_for_idle().await;
    assert_eq!(wait_persisted(&session, NOTE, 1).await.len(), 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        faux.call_count(),
        1,
        "the run was not extended by a continuation"
    );

    let _ = session
        .prompt(UserInput::text("next", InputSource::Sdk))
        .await
        .unwrap();
    session.wait_for_idle().await;
    let requests = requests.lock().unwrap().clone();
    let texts = user_texts(&requests[1]);
    assert_eq!(count_of(&texts, "background notice"), 1, "{texts:?}");
    assert!(
        position_of(&texts, "background notice").unwrap() < position_of(&texts, "next").unwrap()
    );
}

/// A native extension that, on the FIRST `agent_settled`, injects a `deliverAs: "steer"` message
/// and holds the emit open until that message is in the session tree, recording whether it got
/// there while the emit was still running — with nothing handed to the agent's steering queue.
struct SettleInjector {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    session: Arc<OnceLock<Weak<AgentSession>>>,
    fired: AtomicBool,
    appended_during_emit: AtomicBool,
}

#[async_trait::async_trait]
impl NativeExtension for SettleInjector {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("settle-injector")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentSettled]);
        Ok(())
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::AgentSettled) && !self.fired.swap(true, Ordering::SeqCst) {
            let services = self.services.lock().unwrap().clone().unwrap();
            services
                .inject_message_steer("settle-time peer note", Some(NOTE), true, None)
                .unwrap();
            let Some(session) = self.session.get().and_then(Weak::upgrade) else {
                return HookOutcome::Noop;
            };
            let appended = wait_persisted(&session, NOTE, 1).await.len() == 1;
            self.appended_during_emit.store(
                appended && !session.is_run_active() && !session.has_queued_messages(),
                Ordering::SeqCst,
            );
        }
        HookOutcome::Noop
    }
}

/// ICOM-035's constraint — restoring the steer must not reintroduce the lost-steer race the pump
/// was built to close. A steer handed to a run that never polls its steering queue again stays in
/// the agent's queue with the session idle; the pump takes it back at the idle edge and appends it
/// to the tree and transcript. It reaches the next request exactly once — not twice (once from the
/// queue and once from the append) and not zero times.
///
/// The window is reached through an abort: the steer is queued while the run's only request is in
/// flight, and the request is then aborted. An aborted response is a hard exit that polls nothing
/// (pi `agent-loop.ts:244-254`), and pi's `abort()` leaves the agent's queues alone
/// (`agent-session.ts:2075-2085` @v0.87.1), so the steer is past the run's last poll.
///
/// It used to be reached from an `agent_settled` handler instead. SEAM-129 made that emit run with
/// the run latch already released, as pi's `_emitAgentSettled` does (`agent-session.ts:870-873`
/// @v0.87.1), so a steer injected there is no longer steered at all — see the next test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_landing_past_the_last_poll_is_delivered_once_at_the_idle_edge() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let faux = Arc::new(FauxProvider::new());
    let held = {
        let requests = requests.clone();
        // The run's only request is held open until the run is aborted, so the steer below is
        // queued before its end and no later request can drain it.
        FauxResponseStep::async_factory(move |ctx: Context, options: StreamOptions, _, _| {
            let requests = requests.clone();
            async move {
                requests.lock().unwrap().push(ctx.messages.clone());
                options
                    .cancel
                    .expect("a run's request carries the run's cancel token")
                    .cancelled()
                    .await;
                faux_assistant_message(vec![faux_text("never delivered")], StopReason::Stop)
            }
        })
    };
    faux.set_response_steps(vec![held, recording_text(&requests, "second done")]);
    let probe = probe();
    let session = build(&fx, &faux, Some(probe.clone())).await;
    let services = probe.services.lock().unwrap().clone().unwrap();

    let _ = session
        .prompt(UserInput::text("first", InputSource::Sdk))
        .await
        .unwrap();
    assert!(within(Duration::from_secs(10), || faux.call_count() >= 1).await);
    services
        .inject_message_steer("late peer note", Some(NOTE), true, None)
        .unwrap();
    assert!(
        within(Duration::from_secs(10), || session.has_queued_messages()).await,
        "the injection was steered onto the live run"
    );
    session.abort();
    session.wait_for_idle().await;
    let bodies = wait_persisted(&session, NOTE, 1).await;
    assert_eq!(
        bodies.len(),
        1,
        "the stranded steer was appended at the idle edge"
    );
    assert!(
        !session.has_queued_messages(),
        "and taken OUT of the agent's steering queue, so no later run drains it a second time"
    );
    assert_eq!(faux.call_count(), 1, "no turn was started for it");

    let _ = session
        .prompt(UserInput::text("next", InputSource::Sdk))
        .await
        .unwrap();
    session.wait_for_idle().await;
    let requests = requests.lock().unwrap().clone();
    let texts = user_texts(&requests[1]);
    assert_eq!(
        count_of(&texts, "late peer note"),
        1,
        "exactly once: {texts:?}"
    );
    assert!(position_of(&texts, "late peer note").unwrap() < position_of(&texts, "next").unwrap());
    assert_eq!(persisted(&session, NOTE).await.len(), 1);
}

/// SEAM-129 at the injection seam — pi's `_emitAgentSettled` clears `_isAgentRunActive` BEFORE the
/// emit (`agent-session.ts:870-873` @v0.87.1), so a `deliverAs: "steer"` message sent from an
/// `agent_settled` handler meets a session that is not streaming and takes `sendCustomMessage`'s
/// plain `_appendCustomMessage` arm (`:1964-1966`): it is in the tree while the emit is still
/// running, nothing is handed to the agent's steering queue, and no turn follows. When the emit
/// still ran under the run latch, the pump steered it onto the finished run and it reached the tree
/// only after the emit, at the idle edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_sent_from_agent_settled_is_appended_during_the_emit() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        recording_text(&requests, "first done"),
        recording_text(&requests, "second done"),
    ]);
    let slot = Arc::new(OnceLock::new());
    let injector = Arc::new(SettleInjector {
        services: Mutex::new(None),
        session: slot.clone(),
        fired: AtomicBool::new(false),
        appended_during_emit: AtomicBool::new(false),
    });
    let session = build(&fx, &faux, Some(injector.clone())).await;
    let _ = slot.set(Arc::downgrade(&session));

    let _ = session
        .prompt(UserInput::text("first", InputSource::Sdk))
        .await
        .unwrap();
    session.wait_for_idle().await;
    assert!(
        injector.appended_during_emit.load(Ordering::SeqCst),
        "the message was appended while agent_settled was being emitted, not steered"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(faux.call_count(), 1, "no turn was started for it");
    assert_eq!(persisted(&session, NOTE).await.len(), 1);

    let _ = session
        .prompt(UserInput::text("next", InputSource::Sdk))
        .await
        .unwrap();
    session.wait_for_idle().await;
    let requests = requests.lock().unwrap().clone();
    let texts = user_texts(&requests[1]);
    assert_eq!(
        count_of(&texts, "settle-time peer note"),
        1,
        "exactly once: {texts:?}"
    );
    assert!(
        position_of(&texts, "settle-time peer note").unwrap()
            < position_of(&texts, "next").unwrap()
    );
}

/// The summarization system prompt's opening words (`cyrup-session` `SUMMARIZATION_SYSTEM_PROMPT`):
/// how the router below tells a compaction's summarizer call from a model turn.
const SUMMARIZER_PROMPT_HEAD: &str = "You are a context summarization assistant";

type Reply =
    std::pin::Pin<Box<dyn std::future::Future<Output = cyrup_core::AssistantMessage> + Send>>;

/// A provider that routes by request KIND rather than by call order: a compaction may make one or
/// two summarizer calls (history, plus a split turn's prefix), so a fixed step list cannot say
/// which call is which. Summarizer calls go to `summarize`; model turns are recorded in `requests`
/// and answered by `turn(n)` for the n-th model turn.
fn routed(
    requests: &Requests,
    turn: impl Fn(usize) -> cyrup_core::AssistantMessage + Send + Sync + 'static,
    summarize: impl Fn() -> Reply + Send + Sync + 'static,
) -> Arc<FauxProvider> {
    let turn = Arc::new(turn);
    let summarize = Arc::new(summarize);
    let requests = requests.clone();
    let step = FauxResponseStep::async_factory(move |ctx: Context, _, _, _| {
        let is_summary = ctx
            .system_prompt
            .as_deref()
            .is_some_and(|p| p.starts_with(SUMMARIZER_PROMPT_HEAD));
        let reply: Reply = if is_summary {
            summarize()
        } else {
            let n = {
                let mut requests = requests.lock().unwrap();
                requests.push(ctx.messages.clone());
                requests.len() - 1
            };
            let message = turn(n);
            Box::pin(async move { message })
        };
        reply
    });
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![step; 32]);
    faux
}

/// A native extension that captures the session's `HostServices` and records whether the session
/// still reported a compaction in flight when `session_compact` was delivered to it.
struct CompactWatcher {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    session: OnceLock<Weak<AgentSession>>,
    compacting_at_session_compact: Mutex<Option<bool>>,
}

#[async_trait::async_trait]
impl NativeExtension for CompactWatcher {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("compact-watcher")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionCompact]);
        Ok(())
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::SessionCompact { .. })
            && let Some(session) = self.session.get().and_then(Weak::upgrade)
        {
            *self.compacting_at_session_compact.lock().unwrap() = Some(session.is_compacting());
        }
        HookOutcome::Noop
    }
}

/// SEAM-125 + ICOM-062 step 3 — a manual compaction is busy: `is_idle()` false while
/// `is_run_active()` stays false (pi's `isIdle` vs `isStreaming`), the extension-facing
/// `HostServices::is_idle` agrees, `wait_for_idle()` resolves only after `compaction_end`, and an
/// injected turn arriving mid-compaction starts NO model run until the compaction is over — then
/// exactly one, over the compacted context plus the message.
///
/// The compaction's idle edge is where that held turn starts, so it must come after the agent
/// transcript is re-seeded from the compacted context. pi clears its compaction state only after
/// `appendCompaction` → `_refreshFinalizedContext()` and the `session_compact` emit
/// (`agent-session.ts:2499-2528` @v0.87.1); an edge raised ahead of the re-seed let the held turn
/// start over the pre-compaction transcript whenever the pump won that race. The watcher pins the
/// order without depending on who wins it: the session is still compacting while `session_compact`
/// is delivered, which comes after the re-seed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manual_compaction_is_busy_and_holds_an_injected_turn_until_it_ends() {
    let fx = fixture();
    let requests: Requests = Arc::default();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let faux = routed(
        &requests,
        |n| faux_assistant_message(vec![faux_text(format!("turn {n}"))], StopReason::Stop),
        move || {
            let mut released = release_rx.clone();
            Box::pin(async move {
                let _ = released.wait_for(|r| *r).await;
                faux_assistant_message(vec![faux_text("SUMMARY-OF-EARLIER-WORK")], StopReason::Stop)
            })
        },
    );
    let watcher = Arc::new(CompactWatcher {
        services: Mutex::new(None),
        session: OnceLock::new(),
        compacting_at_session_compact: Mutex::new(None),
    });
    let session = build(&fx, &faux, Some(watcher.clone())).await;
    let _ = watcher.session.set(Arc::downgrade(&session));
    let services = watcher.services.lock().unwrap().clone().unwrap();
    for turn in ["one", "two"] {
        let _ = session
            .prompt(UserInput::text(turn, InputSource::Sdk))
            .await
            .unwrap();
        session.wait_for_idle().await;
    }
    let model_turns = || requests.lock().unwrap().len();
    assert_eq!(model_turns(), 2);

    let compaction = {
        let session = session.clone();
        tokio::spawn(async move { session.compact(None).await })
    };
    assert!(
        within(Duration::from_secs(10), || faux.call_count() > 2).await,
        "the summarizer request is in flight"
    );
    assert!(session.is_compacting());
    assert!(
        !session.is_run_active(),
        "a compaction is not a run (pi `isStreaming`)"
    );
    assert!(!session.is_idle(), "but it is busy (pi v0.85.1 `isIdle`)");
    assert!(
        !services.is_idle(),
        "extensions read the same answer through ctx.isIdle()"
    );

    let waited = Arc::new(AtomicBool::new(false));
    let waiter = {
        let session = session.clone();
        let waited = waited.clone();
        tokio::spawn(async move {
            session.wait_for_idle().await;
            waited.store(true, Ordering::SeqCst);
        })
    };
    session
        .inject_message(
            "peer during compaction".into(),
            Some(NOTE.into()),
            true,
            None,
            true,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !waited.load(Ordering::SeqCst),
        "wait_for_idle waits the compaction out"
    );
    assert_eq!(model_turns(), 2, "no model run starts under the compaction");

    release_tx.send(true).unwrap();
    let result = compaction.await.unwrap().expect("the compaction succeeds");
    assert!(result.summary.contains("SUMMARY-OF-EARLIER-WORK"));
    assert_eq!(
        *watcher.compacting_at_session_compact.lock().unwrap(),
        Some(true),
        "the compaction's idle edge follows the transcript re-seed and session_compact"
    );
    tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .expect("wait_for_idle resolves once the compaction has ended")
        .unwrap();
    assert!(
        within(Duration::from_secs(10), || model_turns() >= 3).await,
        "the injected turn ran after the compaction"
    );
    session.wait_for_idle().await;
    let requests_now = requests.lock().unwrap().clone();
    let texts = user_texts(&requests_now[2]);
    assert!(
        texts.iter().any(|t| t.contains("SUMMARY-OF-EARLIER-WORK")),
        "the turn ran over the COMPACTED context: {texts:?}"
    );
    assert_eq!(
        texts.last().map(|t| t.contains("peer during compaction")),
        Some(true),
        "with the injected message as its input: {texts:?}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(model_turns(), 3, "exactly one turn");
}

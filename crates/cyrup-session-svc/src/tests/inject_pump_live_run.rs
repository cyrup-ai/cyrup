//! SUBA-229 — the injection pump routes a turn-triggering request onto the LIVE run, as pi's
//! `sendCustomMessage` does while `isStreaming` (branch 2: `this.agent.steer(appMessage)`,
//! `agent-session.ts:1949-1954` @v0.87.1), instead of holding it until the session is idle and
//! starting a run of its own; a message the run did not drain is reclaimed and run exactly once.
//!
//! "Runs" are counted as `agent_settled` events: pi emits exactly one per `_runAgentPrompt`,
//! continuations included. Each test names the mutation it kills. Waits are on conditions (the
//! pump's [`crate::session::queued_turn::LiveTurnProbe`], a provider entry signal), bounded so that
//! a pump that never queues fails the test's assertions rather than hanging it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use cyrup_agent::AgentMessage;
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::host::InjectOutcome;
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use futures::{FutureExt, StreamExt};
use serde_json::json;
use tempfile::TempDir;

use crate::{
    AgentSession, AgentSessionEvent, InputSource, SessionBuilder, SessionConfig, UserInput,
};

const KIND: &str = "subagent-notify";
/// How long a condition wait may take before the test goes on to fail its own assertions.
const BOUND: Duration = Duration::from_secs(5);

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
    std::fs::write(cwd.join("a.txt"), "file body").unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

async fn build(
    fx: &Fixture,
    faux: &Arc<FauxProvider>,
    ext: Option<Arc<dyn NativeExtension>>,
) -> Arc<AgentSession> {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let mut builder = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg);
    if let Some(ext) = ext {
        builder = builder.with_native_extension(ext);
    }
    builder.build().await.unwrap().into_shared()
}

/// Every transcript custom message of [`KIND`], as its body.
async fn notices(session: &AgentSession) -> Vec<String> {
    session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            AgentMessage::Custom { kind, payload, .. } if kind == KIND => {
                Some(payload.as_str().unwrap_or_default().to_string())
            }
            _ => None,
        })
        .collect()
}

/// Wait (bounded, not asserted) until the pump holds at least `n` requests in turns queued onto
/// the live run.
async fn until_held(session: &AgentSession, n: usize) {
    let mut held = session.live_turn_probe.held.subscribe();
    let _ = tokio::time::timeout(BOUND, held.wait_for(|h| *h >= n)).await;
}

/// The SUBA-187 count: turn-triggering injections queued and not yet answered.
fn owed_turns(session: &AgentSession) -> usize {
    *session
        .services()
        .host_services
        .pending_turn_injections()
        .subscribe()
        .borrow()
}

/// Every event already on `events`, without waiting for more.
fn drain(events: &mut cyrup_core::EventStream<AgentSessionEvent>) -> Vec<AgentSessionEvent> {
    let mut out = Vec::new();
    while let Some(Some(ev)) = events.next().now_or_never() {
        out.push(ev);
    }
    out
}

fn settles(events: &[AgentSessionEvent]) -> usize {
    events
        .iter()
        .filter(|ev| matches!(ev, AgentSessionEvent::AgentSettled { .. }))
        .count()
}

fn inject_turn(
    session: &AgentSession,
    body: &str,
) -> tokio::sync::oneshot::Receiver<InjectOutcome> {
    session
        .services()
        .host_services
        .inject_message_ack(body, Some(KIND), true, None, true)
        .expect("the pump is bound on a shared session")
}

/// A provider whose first call signals `entered`, parks until `gate` is notified and then answers
/// `first`; later calls answer from `rest`. Every request's context is recorded as text.
struct Gated {
    faux: Arc<FauxProvider>,
    entered: Arc<tokio::sync::Notify>,
    gate: Arc<tokio::sync::Notify>,
    contexts: Arc<Mutex<Vec<String>>>,
}

fn gated_provider(
    first: cyrup_core::AssistantMessage,
    rest: Vec<cyrup_core::AssistantMessage>,
) -> Gated {
    let faux = Arc::new(FauxProvider::new());
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let mut steps = Vec::new();
    {
        let (entered, gate, contexts) = (entered.clone(), gate.clone(), contexts.clone());
        steps.push(FauxResponseStep::async_factory(move |ctx, _, _, _| {
            contexts.lock().unwrap().push(format!("{ctx:?}"));
            let (entered, gate, first) = (entered.clone(), gate.clone(), first.clone());
            async move {
                entered.notify_one();
                gate.notified().await;
                first
            }
        }));
    }
    for answer in rest {
        let contexts = contexts.clone();
        steps.push(FauxResponseStep::factory(move |ctx, _, _, _| {
            contexts.lock().unwrap().push(format!("{ctx:?}"));
            answer.clone()
        }));
    }
    faux.set_response_steps(steps);
    Gated {
        faux,
        entered,
        gate,
        contexts,
    }
}

/// An `agent_end` handler that sends a turn-triggering completion — the shape of the subagent
/// extension's headless drain (SUBA-187) — and holds the post-`agent_end` gap open until the pump
/// has queued it onto the run (bounded: a pump that never queues times the wait out).
#[derive(Default)]
struct AgentEndSender {
    services: OnceLock<Arc<dyn HostServices>>,
    session: OnceLock<Weak<AgentSession>>,
    ack: Mutex<Option<tokio::sync::oneshot::Receiver<InjectOutcome>>>,
}

#[async_trait::async_trait]
impl NativeExtension for AgentEndSender {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("suba-229-agent-end-sender")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentEnd]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::AgentEnd { .. } = ev else {
            return HookOutcome::Noop;
        };
        {
            let mut slot = self.ack.lock().unwrap();
            if slot.is_some() {
                return HookOutcome::Noop;
            }
            let Some(services) = self.services.get() else {
                return HookOutcome::Noop;
            };
            *slot = services
                .inject_message_ack("late completion", Some(KIND), true, None, true)
                .ok();
        }
        if let Some(session) = self.session.get().and_then(Weak::upgrade) {
            until_held(&session, 1).await;
        }
        HookOutcome::Noop
    }
}

/// (a) A turn-triggering injection that arrives in the post-`agent_end` gap joins the live run:
/// `_handlePostAgentRun` continues for the queued message, so ONE run carries both turns, and the
/// message is in that run's transcript before it settles.
///
/// Killing mutation: remove the `queue_turn_injections_onto_live_run` call from the pump's busy
/// phase (main before SUBA-229): the pump holds the message until idle and starts a second run —
/// two `agent_settled`, the message after the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_injection_in_the_post_agent_end_gap_joins_the_live_run() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("user turn done")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("saw the completion")], StopReason::Stop),
    ]);
    let ext = Arc::new(AgentEndSender::default());
    let session = build(&fx, &faux, Some(ext.clone() as Arc<dyn NativeExtension>)).await;
    let _ = ext.session.set(Arc::downgrade(&session));
    let mut events = session.subscribe();

    let _stream = session
        .prompt(UserInput::text("start", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;

    let seen = drain(&mut events);
    assert_eq!(
        settles(&seen),
        1,
        "ONE run carried the completion: {seen:#?}"
    );
    assert_eq!(
        faux.call_count(),
        2,
        "the user's turn and the completion's turn"
    );
    let notice_end = seen.iter().position(|ev| {
        matches!(ev, AgentSessionEvent::MessageEnd { message: AgentMessage::Custom { kind, .. } } if kind == KIND)
    });
    let settled_at = seen
        .iter()
        .position(|ev| matches!(ev, AgentSessionEvent::AgentSettled { .. }));
    assert!(
        matches!((notice_end, settled_at), (Some(n), Some(s)) if n < s),
        "the completion is in the run's transcript before it settles: {seen:#?}"
    );
    assert_eq!(notices(&session).await, vec!["late completion".to_string()]);
    let ack = ext.ack.lock().unwrap().take().expect("the handler sent");
    assert_eq!(ack.await.ok(), Some(InjectOutcome::Accepted));
    assert_eq!(owed_turns(&session), 0, "the SUBA-187 count is released");
}

/// (b) A completion that arrives while the run is inside a tool loop is delivered in the SAME run,
/// at its next steering boundary (after the tool batch): the request that follows the tool result
/// carries it. pi-subagents' completions carry no `deliverAs` (`notify.ts:631-639` @ad11b7ab), so
/// pi's branch 2 steers them.
///
/// Killing mutation: as (a) — the completion then waits for idle and runs as a third request in a
/// second run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completion_during_a_tool_loop_is_delivered_in_the_same_run() {
    let fx = fixture();
    let g = gated_provider(
        faux_assistant_message(
            vec![faux_tool_call("read", json!({ "path": "a.txt" }))],
            StopReason::ToolUse,
        ),
        vec![
            faux_assistant_message(vec![faux_text("read it")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("a second run")], StopReason::Stop),
        ],
    );
    let session = build(&fx, &g.faux, None).await;
    let mut events = session.subscribe();
    let _stream = session
        .prompt(UserInput::text("start", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    g.entered.notified().await;

    let ack = inject_turn(&session, "c-during-tools");
    until_held(&session, 1).await;
    g.gate.notify_one();
    session.wait_for_idle().await;

    let seen = drain(&mut events);
    assert_eq!(settles(&seen), 1, "one run: {seen:#?}");
    assert_eq!(
        g.faux.call_count(),
        2,
        "the tool turn and the turn after it"
    );
    let contexts = g.contexts.lock().unwrap().clone();
    assert!(
        contexts[1].contains("c-during-tools") && contexts[1].contains("file body"),
        "the request after the tool batch carries the tool result AND the completion: {contexts:#?}"
    );
    assert_eq!(ack.await.ok(), Some(InjectOutcome::Accepted));
    assert_eq!(owed_turns(&session), 0, "the SUBA-187 count is released");
}

/// (c) A fan-out: three turn-triggering completions sent while the run is busy reach it as ONE
/// message (SUBA-017), so the run answers them in one extra provider call, not three. However the
/// pump's wake-ups split the arrivals, an undrained queued turn is taken back and re-merged with
/// the next arrival ([CYRUP-DELTA]: pi steers each `sendMessage` as it comes).
///
/// Killing mutations: as (a) (two runs); and queueing new arrivals as their own message instead of
/// reclaiming and re-merging (the `merges_with`/`settle_all` step in
/// `queue_turn_injections_onto_live_run`): several notices, more provider calls.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fan_out_queued_together_lands_as_one_message_on_the_live_run() {
    let fx = fixture();
    let g = gated_provider(
        faux_assistant_message(vec![faux_text("user turn done")], StopReason::Stop),
        vec![
            faux_assistant_message(vec![faux_text("saw the fan-out")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("another turn")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("another turn")], StopReason::Stop),
        ],
    );
    let session = build(&fx, &g.faux, None).await;
    let mut events = session.subscribe();
    let _stream = session
        .prompt(UserInput::text("start", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    g.entered.notified().await;

    // One at a time, each queued before the next is sent, so the merge MUST go through the
    // reclaim-and-re-merge path rather than one coalesced inbox.
    let mut acks = Vec::new();
    for (n, body) in ["c1", "c2", "c3"].into_iter().enumerate() {
        acks.push(inject_turn(&session, body));
        until_held(&session, n + 1).await;
    }
    g.gate.notify_one();
    session.wait_for_idle().await;

    let seen = drain(&mut events);
    assert_eq!(settles(&seen), 1, "one run: {seen:#?}");
    assert_eq!(
        g.faux.call_count(),
        2,
        "the user's turn and ONE fan-out turn"
    );
    assert_eq!(
        notices(&session).await,
        vec!["c1\n\nc2\n\nc3".to_string()],
        "one merged message, bodies in arrival order"
    );
    assert!(
        !session.has_queued_messages(),
        "nothing left in the agent's queues"
    );
    for ack in acks {
        assert_eq!(ack.await.ok(), Some(InjectOutcome::Accepted));
    }
    assert_eq!(owed_turns(&session), 0, "the SUBA-187 count is released");
}

/// (d) The race: the run ends between the pump's routing read ("it is streaming") and its push,
/// so the message lands in the agent's steering queue with the session idle. Driven
/// deterministically: the probe's `before_queue` hook parks the pump, after its read, until the
/// run has fully settled (its latches idle). The pump must then take the message back and run it
/// as its own turn — exactly once.
///
/// Killing mutation: `QueuedTurn::take_back` returning `false` (no reclaim): the message is
/// answered `Accepted` and stranded in the queue — never in the transcript.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_queued_as_the_run_ends_is_reclaimed_and_run_exactly_once() {
    let fx = fixture();
    let g = gated_provider(
        faux_assistant_message(vec![faux_text("user turn done")], StopReason::Stop),
        vec![
            faux_assistant_message(vec![faux_text("ran the reclaimed turn")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("a third call")], StopReason::Stop),
        ],
    );
    let session = build(&fx, &g.faux, None).await;
    let mut events = session.subscribe();
    let _stream = session
        .prompt(UserInput::text("start", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    g.entered.notified().await;

    let (fired_tx, fired_rx) = tokio::sync::oneshot::channel::<()>();
    *session.live_turn_probe.before_queue.lock().unwrap() =
        Some(Box::new(move |session: &AgentSession| {
            let _ = fired_tx.send(());
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(session.wait_for_latches_idle());
            });
        }));
    let ack = inject_turn(&session, "raced");
    tokio::time::timeout(BOUND, fired_rx)
        .await
        .expect("the pump reached its push while the run was live")
        .expect("hook fired");
    g.gate.notify_one();
    session.wait_for_idle().await;

    let seen = drain(&mut events);
    assert_eq!(
        settles(&seen),
        2,
        "the user's run, then the pump's own run for the reclaimed message: {seen:#?}"
    );
    assert_eq!(
        notices(&session).await,
        vec!["raced".to_string()],
        "exactly once"
    );
    assert_eq!(g.faux.call_count(), 2, "one call per run");
    assert!(!session.has_queued_messages(), "not stranded in the queue");
    assert_eq!(ack.await.ok(), Some(InjectOutcome::Accepted));
    assert_eq!(owed_turns(&session), 0, "the SUBA-187 count is released");
}

/// pi's branch 3 is `else if (options?.triggerTurn) await this._runAgentPrompt(appMessage)`, for
/// any `deliverAs` but `nextTurn`: a `{ deliverAs: "steer", triggerTurn: true }` message on an
/// idle session runs a turn over it.
///
/// Killing mutation: `send_custom_message`'s `Route::RunTurn` arm folded into the append arm
/// (main before SUBA-229): the message is appended and the provider never called.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_custom_message_with_steer_and_trigger_on_an_idle_session_runs_a_turn() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("answered")],
        StopReason::Stop,
    )]);
    let session = build(&fx, &faux, None).await;
    session
        .send_custom_message(
            KIND,
            json!("run me"),
            true,
            None,
            Some(crate::event::DeliverAs::Steer),
            Some(true),
        )
        .await
        .expect("routed");
    session.wait_for_idle().await;
    assert_eq!(faux.call_count(), 1, "pi's branch 3 ran a turn");
    assert_eq!(notices(&session).await, vec!["run me".to_string()]);
}

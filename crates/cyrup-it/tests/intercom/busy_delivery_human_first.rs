//! ICOM-063 — `busyDelivery: "human-first"` (pi-intercom v0.14.0, `0ce2dcd`), over the parent
//! module's live-session harness: a genuine broker, a peer `IntercomClient`, and a REAL
//! `AgentSession` running the production `IntercomExtension` with `config.json` carrying
//! `"busyDelivery": "human-first"`. The model is scripted so each call parks until the test releases
//! it, which is how upstream's `human-priority.pi.test.ts` measures ordering — from the model's
//! inputs, never from mocked delivery options. Each test below is one of that file's seven cases.
//!
//! What the policy does (README v0.14.0): "busy interactive peers wait outside Pi's queues: one FIFO
//! peer is steered per turn when no human message is pending. If the run ends first, the idle flush
//! releases one via the normal `inboundTrigger` policy; remaining peers wait for later turns."

use super::*;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_session_svc::{PromptOptions, StreamingBehavior};

/// One gate per model call: call `n` parks until [`Releases::release`]`(n)` — upstream's
/// `releaseModel()`. A release may come before the call parks (`send_replace` keeps the value with
/// no receiver, where `send` would drop it).
#[derive(Default)]
struct Releases(Mutex<Vec<tokio::sync::watch::Sender<bool>>>);

impl Releases {
    fn gate(&self, n: usize) -> tokio::sync::watch::Sender<bool> {
        let mut gates = self.0.lock().unwrap();
        while gates.len() <= n {
            gates.push(tokio::sync::watch::channel(false).0);
        }
        gates[n].clone()
    }
    fn release(&self, n: usize) {
        self.gate(n).send_replace(true);
    }
}

fn gated(releases: &Arc<Releases>) -> TurnFn {
    let releases = releases.clone();
    Arc::new(move |n, _| {
        let mut gate = releases.gate(n).subscribe();
        Box::pin(async move {
            let _ = gate.wait_for(|open| *open).await;
            text_reply(&format!("response {}", n + 1))
        })
    })
}

/// Two probes around the intercom extension (one loaded before it, one after), sharing this state.
///
/// * `settled` counts `agent_settled` — upstream's `priority-probe`. A peer that rode the running
///   human run needed no settle boundary; one that got its own triggered turn did.
/// * The `turn_end` pair is test-side synchronisation for ONE host difference, and nothing else:
///   upstream's turn-boundary release reaches `agent.steer` synchronously inside the `turn_end`
///   handler, while cyrup's `inject_message_steer` hands the steer to the live host's injection
///   pump, a separate task. When the first probe saw more held peers than the second one does, the
///   intercom handler released one, and the second probe waits (bounded) until the pump has put it
///   on the agent's steering queue — so each case measures the policy, not the pump's scheduling.
///   Without it the steer can land after the loop's last steering poll under load (~5% of runs at
///   4x process contention), which the result reports as a residual for ICOM-063.
///
///   Consequence: nothing in this file tests SAME-RUN delivery against a free-running pump, and
///   these cases must not be cited as evidence for it. The probe pair and the `yield_now` in
///   `IntercomExtension`'s `TurnEnd` arm go together, in the same change that makes
///   `LiveHostServices::inject_message_steer` steer onto the live agent synchronously while a run
///   is active (area 08; pi `sendCustomMessage`'s streaming arm → `agent.steer`).
#[derive(Default)]
struct Handoff {
    held_at_turn_end: AtomicUsize,
    settled: AtomicUsize,
    session: OnceLock<Weak<AgentSession>>,
    state: OnceLock<Arc<cyrup_intercom::session_state::SharedIntercomState>>,
}

impl Handoff {
    fn count(&self) -> usize {
        self.settled.load(Ordering::SeqCst)
    }
    fn held(&self) -> usize {
        self.state.get().map_or(0, |s| s.held_inbound_len())
    }
}

struct Probe {
    handoff: Arc<Handoff>,
    first: bool,
}

#[async_trait::async_trait]
impl NativeExtension for Probe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(if self.first {
            "priority-probe-first"
        } else {
            "priority-probe"
        })
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::TurnEnd, EventKind::AgentSettled]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::TurnEnd { .. } if self.first => {
                self.handoff
                    .held_at_turn_end
                    .store(self.handoff.held(), Ordering::SeqCst);
            }
            HostEvent::TurnEnd { .. } => {
                let released =
                    self.handoff.held() < self.handoff.held_at_turn_end.load(Ordering::SeqCst);
                let session = self.handoff.session.get().and_then(Weak::upgrade);
                if let (true, Some(session)) = (released, session) {
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                    while !session.has_queued_messages() && tokio::time::Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
            }
            HostEvent::AgentSettled if !self.first => {
                self.handoff.settled.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}

/// Upstream's worker settings: compaction and retry off.
fn plain_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("compaction", serde_json::json!({"enabled": false}))
        .unwrap();
    cli.set_field("retry", serde_json::json!({"enabled": false}))
        .unwrap();
    cli
}

struct Worker {
    live: Live,
    releases: Arc<Releases>,
    settled: Arc<Handoff>,
}

impl Worker {
    async fn start() -> Self {
        let releases = Arc::new(Releases::default());
        let settled = Arc::new(Handoff::default());
        let probe = |first| {
            Some(Arc::new(Probe {
                handoff: settled.clone(),
                first,
            }) as Arc<dyn NativeExtension>)
        };
        let options = Options {
            busy_delivery: Some("human-first"),
            settings: plain_settings(),
            first: probe(true),
            extra: probe(false),
            ..Options::interactive()
        };
        let live = live(options, gated(&releases), no_summaries()).await;
        let _ = settled.session.set(Arc::downgrade(&live.session));
        let _ = settled.state.set(live.ext.state().clone());
        Self {
            live,
            releases,
            settled,
        }
    }

    /// `session.prompt("human task 1")` and `waitForModelCall(1)`, plus the extension having seen
    /// the run's `agent_start` — the state in which a peer would be steered under `"steer"`.
    async fn human_run(&self, text: &str) {
        self.live.prompt(text).await;
        let script = self.live.script.clone();
        let state = self.live.ext.state().clone();
        assert!(
            within(Duration::from_secs(15), || script.model_turns() == 1
                && state.agent_running())
            .await,
            "the human run is in flight"
        );
    }

    async fn queue_human(&self, text: &str, behavior: StreamingBehavior) {
        self.live
            .session
            .prompt_with(
                UserInput::text(text, InputSource::Sdk),
                PromptOptions {
                    streaming_behavior: Some(behavior),
                },
            )
            .await
            .expect("queued onto the running human run");
    }

    /// `waitForModelCall(count)`.
    async fn model_call(&self, count: usize) {
        let script = self.live.script.clone();
        assert!(
            within(Duration::from_secs(15), || script.model_turns() >= count).await,
            "model call {count} never started (got {})",
            self.live.script.model_turns()
        );
    }

    async fn settled(&self, count: usize) {
        let settled = self.settled.clone();
        assert!(
            within(Duration::from_secs(15), || settled.count() >= count).await,
            "agent_settled #{count} never came (got {})",
            self.settled.count()
        );
    }

    fn injected(&self, id: &str) -> bool {
        self.live
            .script
            .receipts(id)
            .iter()
            .any(|s| s == "injected")
    }

    /// The user-role texts of the LAST model request — every human input and every peer card the
    /// run was given, in the order the model saw them.
    fn last_inputs(&self) -> Vec<String> {
        user_texts(self.live.script.requests().last().expect("a model request"))
    }
}

/// `wait_for_idle`, bounded. Every model call here parks until its gate is released, so a stray
/// extra call (a peer that missed its run and got a turn of its own) would otherwise hang the test
/// until the runner's timeout; this turns it into a failure that says what the model was asked.
async fn idle(live: &Live) {
    if tokio::time::timeout(Duration::from_secs(30), live.session.wait_for_idle())
        .await
        .is_err()
    {
        panic!(
            "the session never went idle: {} model call(s), inputs {:?}",
            live.script.model_turns(),
            live.script
                .requests()
                .iter()
                .map(|r| user_texts(r))
                .collect::<Vec<_>>()
        );
    }
}

fn position(texts: &[String], needle: &str) -> usize {
    texts
        .iter()
        .position(|t| t.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} never reached the model: {texts:?}"))
}

/// `busy interactive peer message is held, then steered into the same run at the next turn
/// boundary`. Under `"steer"` the peer would reach the model in call 1's run at the first steering
/// poll; here it is `queued`, stays out of the host's queues, and is steered only at `turn_end`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_peer_is_held_then_steered_into_the_same_run_at_the_next_turn_boundary() {
    let w = Worker::start().await;
    w.human_run("human task 1").await;

    w.live.send("held-peer", "peer while busy", false).await;
    w.live.until_receipt("held-peer", "queued").await;
    assert_eq!(
        w.live.session.pending_message_count(),
        0,
        "a held peer must not enter the steering/follow-up queues"
    );
    assert!(!w.live.session.has_queued_messages());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !w.injected("held-peer"),
        "a held peer is not injected mid-turn"
    );

    w.releases.release(0);
    w.live.until_receipt("held-peer", "injected").await;
    w.model_call(2).await;
    assert_eq!(
        w.settled.count(),
        0,
        "the peer rode the running human run; no settle boundary was needed"
    );
    w.releases.release(1);
    w.settled(1).await;

    let texts = w.last_inputs();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert_eq!(texts[0], "human task 1");
    assert!(texts[1].contains("peer while busy"), "{texts:?}");
    assert_eq!(
        w.live.script.receipts("held-peer"),
        ["receiver_received", "acknowledged", "queued", "injected"]
    );
    assert!(w.live.session.is_idle());
    assert_eq!(w.live.script.model_turns(), 2);
    w.live.shutdown().await;
}

/// `human steer and followUp that arrive after a held peer are processed before the peer`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_human_steer_and_follow_up_arriving_after_a_held_peer_go_first() {
    let w = Worker::start().await;
    w.human_run("human task 1").await;

    w.live
        .send("peer-before-humans", "peer before humans", false)
        .await;
    w.live.until_receipt("peer-before-humans", "queued").await;
    w.queue_human("human steer 2", StreamingBehavior::Steer)
        .await;
    w.queue_human("human follow-up 3", StreamingBehavior::FollowUp)
        .await;
    assert_eq!(
        w.live.session.pending_message_count(),
        2,
        "only the two human messages are in the host's queues"
    );

    w.releases.release(0);
    w.model_call(2).await;
    assert!(
        !w.injected("peer-before-humans"),
        "the peer stays held while the human steer is pending"
    );
    w.releases.release(1);
    w.model_call(3).await;
    assert!(
        !w.injected("peer-before-humans"),
        "the peer stays held while the human follow-up is pending"
    );
    w.releases.release(2);
    w.live.until_receipt("peer-before-humans", "injected").await;
    w.model_call(4).await;
    assert_eq!(
        w.settled.count(),
        0,
        "the peer was steered into the still-running human run, not a new one"
    );
    w.releases.release(3);
    w.settled(1).await;

    let texts = w.last_inputs();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert_eq!(texts[0], "human task 1");
    assert_eq!(texts[1], "human steer 2");
    assert_eq!(texts[2], "human follow-up 3");
    assert!(texts[3].contains("peer before humans"), "{texts:?}");
    assert!(w.live.session.is_idle());
    w.live.shutdown().await;
}

/// `multiple held peers drain one per turn inside the run and human input between them wins`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_peers_drain_one_per_turn_and_human_input_between_them_wins() {
    let w = Worker::start().await;
    w.human_run("human task 1").await;

    w.live.send("peer-a", "peer A", false).await;
    w.live.send("peer-b", "peer B", false).await;
    w.live.until_receipt("peer-a", "queued").await;
    w.live.until_receipt("peer-b", "queued").await;

    w.releases.release(0);
    w.live.until_receipt("peer-a", "injected").await;
    w.model_call(2).await;
    assert!(
        !w.injected("peer-b"),
        "the second peer waits for the next turn boundary: a={:?} b={:?} settled={} requests={:?}",
        w.live.script.receipts("peer-a"),
        w.live.script.receipts("peer-b"),
        w.settled.count(),
        w.live
            .script
            .requests()
            .iter()
            .map(|r| user_texts(r))
            .collect::<Vec<_>>()
    );
    assert_eq!(w.settled.count(), 0, "still the original human run");

    w.queue_human("human steer 2", StreamingBehavior::Steer)
        .await;
    w.releases.release(1);
    w.model_call(3).await;
    assert!(!w.injected("peer-b"), "the human steer goes first");
    w.releases.release(2);
    w.live.until_receipt("peer-b", "injected").await;
    w.model_call(4).await;
    w.releases.release(3);
    w.settled(1).await;

    let texts = w.last_inputs();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert_eq!(texts[0], "human task 1");
    assert!(texts[1].contains("peer A"), "{texts:?}");
    assert_eq!(texts[2], "human steer 2");
    assert!(texts[3].contains("peer B"), "{texts:?}");
    assert!(w.live.session.is_idle());
    w.live.shutdown().await;
}

/// `held peers hand off one triggered turn when a busy run ends without a turn boundary`: the run
/// is aborted, so its `turn_end` (`stopReason: "aborted"`) cannot take a steer; the idle flush then
/// releases ONE peer as a triggered turn, and the second rides that turn's boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_peers_hand_off_one_triggered_turn_when_the_run_ends_without_a_turn_boundary() {
    let w = Worker::start().await;
    w.human_run("human task").await;
    w.live.send("handoff-a", "first peer", false).await;
    w.live.send("handoff-b", "second peer", false).await;
    w.live.until_receipt("handoff-b", "queued").await;

    w.live.session.abort();
    w.releases.release(0);
    w.live.until_receipt("handoff-a", "injected").await;
    w.model_call(2).await;
    assert!(
        !w.injected("handoff-b"),
        "the idle flush releases one peer, not the queue"
    );
    w.releases.release(1);
    w.live.until_receipt("handoff-b", "injected").await;
    w.model_call(3).await;
    w.releases.release(2);
    w.settled(2).await;

    let requests = w.live.script.requests();
    let second = user_texts(&requests[1]);
    assert!(
        mentions(&requests[1], "first peer") == 1 && mentions(&requests[1], "second peer") == 0,
        "the handoff turn carries the first peer only: {second:?}"
    );
    let third = user_texts(&requests[2]);
    assert!(
        position(&third, "first peer") < position(&third, "second peer"),
        "{third:?}"
    );
    w.live.shutdown().await;
}

/// `a held ask answered during the busy run is dropped with an acknowledged receipt, not
/// injected` — answered through the real `intercom{reply}` tool, mid-run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_ask_answered_mid_run_is_dropped_with_acknowledged_not_injected() {
    let w = Worker::start().await;
    w.human_run("human task 1").await;
    w.live.send("early-ask", "Early question?", true).await;
    w.live.until_receipt("early-ask", "queued").await;

    let tool = IntercomTool::new(w.live.ext.state().clone());
    let out = cyrup_core::Tool::execute(
        &tool,
        cyrup_core::ToolCallId::from("early-reply"),
        serde_json::json!({ "action": "reply", "replyTo": "early-ask", "message": "Answered early." }),
        cyrup_core::CancelToken::new(),
        Box::new(|_| {}) as cyrup_core::ToolUpdateSink,
    )
    .await
    .expect("the reply is delivered");
    assert_eq!(
        out.details.as_ref().and_then(|d| d.get("delivered")),
        Some(&serde_json::json!(true)),
        "{:?}",
        out.details
    );
    let script = w.live.script.clone();
    assert!(
        within(Duration::from_secs(15), || script
            .receipts("early-ask")
            .len()
            == 4)
        .await,
        "{:?}",
        w.live.script.receipts("early-ask")
    );

    w.releases.release(0);
    w.settled(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        w.live.script.model_turns(),
        1,
        "no handoff turn for the answered ask"
    );
    assert!(
        w.live
            .script
            .requests()
            .iter()
            .all(|r| mentions(r, "Early question?") == 0),
        "the answered ask never reached the model"
    );
    assert_eq!(
        w.live.script.receipts("early-ask"),
        [
            "receiver_received",
            "acknowledged",
            "queued",
            "acknowledged"
        ]
    );
    w.live.shutdown().await;
}

/// `cancelled and superseded held peers are dropped before injection`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_and_superseded_held_peers_are_dropped_before_injection() {
    let w = Worker::start().await;
    w.human_run("human task 1").await;

    w.live.send("cancel-me", "cancel me", true).await;
    w.live.send("old-version", "old version", false).await;
    w.live.until_receipt("cancel-me", "queued").await;
    w.live.until_receipt("old-version", "queued").await;
    assert!(
        w.live
            .peer
            .cancel_message("cancel-me")
            .await
            .expect("the cancel is routed")
            .delivered
    );
    w.live.until_receipt("cancel-me", "cancelled").await;
    let sent = w
        .live
        .peer
        .send(
            &w.live.target,
            SendOptions {
                text: "new version".to_string(),
                message_id: Some("new-version".to_string()),
                supersedes: Some("old-version".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("the replacement is accepted");
    assert!(sent.delivered);
    w.live.until_receipt("old-version", "superseded").await;
    w.live.until_receipt("new-version", "queued").await;

    w.releases.release(0);
    w.live.until_receipt("new-version", "injected").await;
    w.model_call(2).await;
    w.releases.release(1);
    w.settled(1).await;
    assert!(w.live.session.is_idle());

    let requests = w.live.script.requests();
    assert_eq!(requests.len(), 2);
    let texts = user_texts(&requests[1]);
    assert!(texts.iter().any(|t| t.contains("new version")), "{texts:?}");
    assert!(
        requests
            .iter()
            .all(|r| mentions(r, "cancel me") == 0 && mentions(r, "old version") == 0),
        "dropped entries never reach the model"
    );
    assert_eq!(
        w.live.script.receipts("cancel-me"),
        ["receiver_received", "acknowledged", "queued", "cancelled"]
    );
    assert_eq!(
        w.live.script.receipts("old-version"),
        ["receiver_received", "acknowledged", "queued", "superseded"]
    );
    w.live.shutdown().await;
}

/// `held peers expire on session shutdown instead of being injected later` — `session_shutdown`
/// is emitted to the extension while the human run is still in flight, as upstream does, and only
/// then is the model released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_peers_expire_on_session_shutdown_instead_of_being_injected_later() {
    let w = Worker::start().await;
    w.human_run("human task 1").await;
    w.live.send("expire-me", "expire me", false).await;
    w.live.until_receipt("expire-me", "queued").await;

    let ctx = HostCtx::event(cyrup_ext::ExtMode::Tui, true, w.live.cwd.clone());
    let _ = w
        .live
        .ext
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "quit".to_string(),
                target_session_file: None,
            },
            &ctx,
        )
        .await;
    w.releases.release(0);
    idle(&w.live).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        w.live.script.receipts("expire-me"),
        ["receiver_received", "acknowledged", "queued", "expired"]
    );
    assert_eq!(w.live.script.model_turns(), 1);
    assert!(w.live.persisted().await.is_empty(), "never injected");
    w.live.shutdown().await;
}

/// The default is unchanged: with no `busyDelivery` key the same busy peer is steered at once — it
/// is never `queued` — which is what makes every case above an effect of the opt-in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_opt_in_a_busy_peer_is_steered_not_held() {
    let releases = Arc::new(Releases::default());
    let options = Options {
        settings: plain_settings(),
        ..Options::interactive()
    };
    let live = live(options, gated(&releases), no_summaries()).await;
    live.prompt("human task 1").await;
    let script = live.script.clone();
    let state = live.ext.state().clone();
    assert!(
        within(Duration::from_secs(15), || script.model_turns() == 1
            && state.agent_running())
        .await
    );
    live.send("steered-peer", "peer while busy", false).await;
    assert_eq!(
        live.until_receipt("steered-peer", "injected").await,
        ["receiver_received", "acknowledged", "injected"]
    );
    // Two calls are expected; the spare gates let a stray third call finish, so it fails the
    // request-count assertion below with the inputs rather than parking the session forever.
    for n in 0..4 {
        releases.release(n);
    }
    idle(&live).await;
    let requests = live.script.requests();
    assert_eq!(
        requests.len(),
        2,
        "{:?}",
        requests.iter().map(|r| user_texts(r)).collect::<Vec<_>>()
    );
    assert_eq!(mentions(&requests[1], "peer while busy"), 1);
    live.shutdown().await;
}

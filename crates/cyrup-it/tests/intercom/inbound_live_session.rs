//! Inbound intercom delivery INTO A LIVE SESSION — the production path end to end: a genuine
//! `cyrup-intercom-broker` process, a peer `IntercomClient` over the real Unix socket, and on the
//! receiving side a REAL `AgentSession` (`SessionBuilder` + `into_shared`, so the injection pump
//! and the post-run driver are live) with the production `IntercomExtension` loaded as a native
//! built-in and bound to the session's own `LiveHostServices`. The model is a scripted faux
//! provider whose requests are captured, so every assertion is about what the MODEL was sent and
//! what the SENDER was told — never about a `HostServices` double that records `inject_message`.
//!
//! That is the blind spot these rows share ("Why the tests stay green"): every earlier test of the
//! busy/steer/no-turn arms stopped at a double's `inject_message` counter and never ran the pump.
//!
//! * **ICOM-035** — a peer message to a session whose agent run is active reaches the running model
//!   at its next steering boundary in the SAME run (pi-intercom v0.14.0 `index.ts:1221-1246`,
//!   `deliverAs: "steer"` → pi `agent.steer`, `agent-session.ts:1949-1954` @v0.87.1).
//! * **ICOM-068** — with `inboundTrigger: "never"` a message is "still delivered, just without
//!   driving a turn": it reaches the model on the user's next prompt.
//! * **ICOM-062** — a message arriving during a manual `/compact` is HELD (`queued` receipt), starts
//!   no run under the compaction, and is delivered with a turn once it is over — for every way a
//!   compaction ends; it is expired by shutdown and by a runtime replacement, dropped by the
//!   sender's cancel, steered when the session turns out to be busy WITH a run, and (non-UI) takes
//!   the busy auto-reply path. Ports of `intercom.integration.test.ts` @v0.14.0 `:2287-2448`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::common::{broker_bin, registration, spawn_broker, within};
use cyrup_core::{AssistantMessage, Content, ExtensionId, Message, StopReason, TerminateHint};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_intercom::config::{config_path, load_config};
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::spawn::wait_for_broker;
use cyrup_provider::faux::{
    FauxMessageOptions, FauxProvider, FauxResponseStep, faux_assistant_message,
    faux_assistant_message_with, faux_text, faux_tool_call,
};
use cyrup_provider::{Context, Provider};
use cyrup_session_svc::{
    AgentSession, AppMode, InputSource, SessionBuilder, SessionConfig, SessionServiceError,
    UserInput,
};
use tempfile::TempDir;

const INTERCOM_TYPE: &str = "intercom_message";
/// The summarization system prompt's opening words (`cyrup-session` `SUMMARIZATION_SYSTEM_PROMPT`).
const SUMMARIZER_PROMPT_HEAD: &str = "You are a context summarization assistant";
const SUMMARY: &str = "SUMMARY-OF-EARLIER-WORK";

type Reply = std::pin::Pin<Box<dyn std::future::Future<Output = AssistantMessage> + Send>>;
type TurnFn = Arc<dyn Fn(usize, Vec<Message>) -> Reply + Send + Sync>;
type SummaryFn = Arc<dyn Fn() -> Reply + Send + Sync>;

fn ready(message: AssistantMessage) -> Reply {
    Box::pin(async move { message })
}

fn text_reply(text: &str) -> AssistantMessage {
    faux_assistant_message(vec![faux_text(text.to_string())], StopReason::Stop)
}

fn read_tool_call() -> AssistantMessage {
    faux_assistant_message(
        vec![faux_tool_call(
            "read",
            serde_json::json!({ "path": "notes.txt" }),
        )],
        StopReason::ToolUse,
    )
}

/// The text of every USER-role message in a request, in order (a custom message reaches the
/// provider as a user message carrying its content — pi `convertToLlm`'s `case "custom"`).
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

fn mentions(messages: &[Message], needle: &str) -> usize {
    user_texts(messages)
        .iter()
        .filter(|t| t.contains(needle))
        .count()
}

/// Everything the harness scripts and records.
#[derive(Default)]
struct Script {
    /// Model-turn requests (summarizer calls excluded), in call order.
    requests: Mutex<Vec<Vec<Message>>>,
    summarizer_calls: AtomicUsize,
    /// What the peer was told about each message it sent: receipt statuses, in arrival order.
    receipts: Mutex<HashMap<String, Vec<String>>>,
    /// Messages the peer received (the busy auto-reply).
    peer_inbox: Mutex<Vec<cyrup_intercom::transport::protocol::Message>>,
}

impl Script {
    fn model_turns(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }
    fn receipts(&self, id: &str) -> Vec<String> {
        self.receipts
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default()
    }
}

/// A provider that routes by request KIND: a compaction makes one or two summarizer calls, so a
/// fixed step list cannot say which call is which.
fn routed(script: &Arc<Script>, turn: TurnFn, summarize: SummaryFn) -> Arc<FauxProvider> {
    let script = script.clone();
    let step = FauxResponseStep::async_factory(move |ctx: Context, _, _, _| {
        let is_summary = ctx
            .system_prompt
            .as_deref()
            .is_some_and(|p| p.starts_with(SUMMARIZER_PROMPT_HEAD));
        if is_summary {
            script.summarizer_calls.fetch_add(1, Ordering::SeqCst);
            return summarize();
        }
        let n = {
            let mut requests = script.requests.lock().unwrap();
            requests.push(ctx.messages.clone());
            requests.len() - 1
        };
        turn(n, ctx.messages)
    });
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![step; 64]);
    faux
}

/// A gate a scripted response parks on until the test opens it.
#[derive(Clone)]
struct Gate(tokio::sync::watch::Sender<bool>);

impl Gate {
    fn new() -> Self {
        Self(tokio::sync::watch::channel(false).0)
    }
    fn open(&self) {
        let _ = self.0.send(true);
    }
    async fn passed(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|open| *open).await;
    }
}

/// A `session_before_compact` handler that parks until released, then vetoes — the compaction
/// "cancel" outcome, held open long enough for a peer message to arrive under it.
struct HeldVeto {
    entered: AtomicBool,
    gate: Gate,
}

#[async_trait::async_trait]
impl NativeExtension for HeldVeto {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("held-veto")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionBeforeCompact]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionBeforeCompact { .. } => {
                self.entered.store(true, Ordering::SeqCst);
                self.gate.passed().await;
                HookOutcome::Block {
                    reason: Some("not now".to_string()),
                    terminate: TerminateHint::Unspecified,
                }
            }
            _ => HookOutcome::Noop,
        }
    }
}

struct Options {
    has_ui: bool,
    inbound_trigger: Option<&'static str>,
    settings: cyrup_config::Settings,
    extra: Option<Arc<dyn NativeExtension>>,
}

impl Options {
    fn interactive() -> Self {
        Self {
            has_ui: true,
            inbound_trigger: None,
            settings: compaction_settings(),
            extra: None,
        }
    }
}

/// Compaction that works on a two-turn session (keep nothing, reserve nothing); retries off so a
/// failing summarizer is one call.
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

/// One retry with a backoff long enough to deliver a message INTO it: the busy-without-a-run window
/// between an errored agent loop's `agent_end` and the retry's `agent_start`.
fn slow_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "retry",
        serde_json::json!({"enabled": true, "maxRetries": 1, "baseDelayMs": 1500}),
    )
    .unwrap();
    cli
}

struct Live {
    _tmp: TempDir,
    cwd: PathBuf,
    broker: tokio::process::Child,
    session: Arc<AgentSession>,
    ext: Arc<IntercomExtension>,
    peer: Arc<IntercomClient>,
    target: String,
    script: Arc<Script>,
}

impl Live {
    async fn send(&self, id: &str, text: &str, expects_reply: bool) {
        let sent = self
            .peer
            .send(
                &self.target,
                SendOptions {
                    text: text.to_string(),
                    message_id: Some(id.to_string()),
                    expects_reply: Some(expects_reply),
                    ..Default::default()
                },
            )
            .await
            .expect("the broker accepts the peer's message");
        assert!(sent.delivered, "the broker delivered {id}");
    }

    async fn prompt(&self, text: &str) {
        let _ = self
            .session
            .prompt(UserInput::text(text, InputSource::Sdk))
            .await
            .expect("prompt accepted");
    }

    async fn persisted(&self) -> Vec<String> {
        use cyrup_session::agent_message::AgentMessage as Raw;
        self.session
            .raw_context_messages()
            .await
            .into_iter()
            .filter_map(|m| match m {
                Raw::Custom(c) if c.custom_type == INTERCOM_TYPE => Some(c.content.to_string()),
                _ => None,
            })
            .collect()
    }

    async fn until_receipt(&self, id: &str, status: &str) -> Vec<String> {
        let script = self.script.clone();
        let ok = within(Duration::from_secs(15), || {
            script.receipts(id).iter().any(|s| s == status)
        })
        .await;
        let got = self.script.receipts(id);
        assert!(ok, "{id}: no `{status}` receipt; got {got:?}");
        got
    }

    async fn shutdown(mut self) {
        self.session.dispose("quit").await;
        self.peer.disconnect();
        let _ = self.broker.kill().await;
    }
}

fn write_config(intercom_dir: &Path, inbound_trigger: Option<&str>) {
    std::fs::create_dir_all(intercom_dir).unwrap();
    let mut body = serde_json::json!({
        "brokerCommand": broker_bin().to_string_lossy(),
        "brokerArgs": [],
    });
    if let Some(trigger) = inbound_trigger {
        body["inboundTrigger"] = serde_json::json!(trigger);
    }
    std::fs::write(config_path(intercom_dir), body.to_string()).unwrap();
}

async fn live(options: Options, turn: TurnFn, summarize: SummaryFn) -> Live {
    let tmp = TempDir::new().unwrap();
    let agent_dir = tmp.path().join("agent");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("notes.txt"), "file contents\n").unwrap();
    let intercom_dir = intercom_dir_path(&agent_dir);
    write_config(&intercom_dir, options.inbound_trigger);
    let socket = broker_socket_path(&intercom_dir);
    let broker = spawn_broker(&agent_dir);
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");

    let script = Arc::new(Script::default());
    let faux = routed(&script, turn, summarize);
    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.clone(),
            cwd.clone(),
            load_config(&intercom_dir).expect("config loads"),
            None,
        )
        .expect("build the intercom extension"),
    );
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.app_mode = if options.has_ui {
        AppMode::Interactive
    } else {
        AppMode::Print
    };
    let mut builder = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .cli_settings(options.settings)
        .with_native_extension(ext.clone());
    if let Some(extra) = options.extra {
        builder = builder.with_native_extension(extra);
    }
    let session = builder.build().await.expect("build").into_shared();
    session.bind_extensions().await;

    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state
            .client()
            .is_some_and(|c| c.is_connected()))
        .await,
        "the live session's intercom connects on session_start"
    );
    let target = state.client().unwrap().session_id().expect("registered id");

    let peer = Arc::new(
        IntercomClient::connect(&socket, registration("peer"), Some("peer-session".into()))
            .await
            .expect("the peer connects"),
    );
    let mut events = peer.subscribe();
    let collector = script.clone();
    tokio::spawn(async move {
        while let Ok(ev) = events.recv().await {
            match ev {
                InboundEvent::MessageReceipt { receipt, .. } => {
                    collector
                        .receipts
                        .lock()
                        .unwrap()
                        .entry(receipt.message_id.clone())
                        .or_default()
                        .push(receipt.status.wire_name().to_string());
                }
                InboundEvent::Message { message, .. } => {
                    collector.peer_inbox.lock().unwrap().push(*message);
                }
                _ => {}
            }
        }
    });
    Live {
        _tmp: tmp,
        cwd,
        broker,
        session,
        ext,
        peer,
        target,
        script,
    }
}

fn answer_every_turn() -> TurnFn {
    Arc::new(|n, _| ready(text_reply(&format!("turn {n}"))))
}

fn no_summaries() -> SummaryFn {
    Arc::new(|| ready(text_reply("unused")))
}

/// ICOM-035 — THE Verify paragraph. A run is held open across two tool calls; a peer message
/// delivered mid-run appears in the provider's SECOND request of the same run, as the intercom
/// custom message; it is persisted once; the peer is told `injected`; and no turn follows the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_session_steers_a_peer_message_into_the_running_model() {
    let slot: Arc<OnceLock<Weak<AgentSession>>> = Arc::new(OnceLock::new());
    let turn: TurnFn = {
        let slot = slot.clone();
        Arc::new(move |n, _| {
            let slot = slot.clone();
            Box::pin(async move {
                match n {
                    // Held until the peer's message has been handed to this run's steering queue,
                    // so it deterministically lands before the run's first steering poll.
                    0 => {
                        let steered = within(Duration::from_secs(15), || {
                            slot.get()
                                .and_then(Weak::upgrade)
                                .is_some_and(|s| s.has_queued_messages())
                        })
                        .await;
                        assert!(steered, "the peer message was never steered onto the run");
                        read_tool_call()
                    }
                    1 => read_tool_call(),
                    _ => text_reply("done"),
                }
            })
        })
    };
    let live = live(Options::interactive(), turn, no_summaries()).await;
    let _ = slot.set(Arc::downgrade(&live.session));

    live.prompt("start the task").await;
    let state = live.ext.state().clone();
    assert!(
        within(Duration::from_secs(10), || live.script.model_turns() == 1
            && state.agent_running())
        .await,
        "the run is in flight and the extension has seen its `agent_start`"
    );
    assert!(live.session.is_run_active());
    live.send("steer-1", "supervisor: stop and use the v2 schema", false)
        .await;
    live.session.wait_for_idle().await;

    let requests = live.script.requests();
    assert_eq!(requests.len(), 3, "one run of three requests");
    assert_eq!(
        mentions(&requests[0], "v2 schema"),
        0,
        "request 1 predates it"
    );
    assert_eq!(
        mentions(&requests[1], "v2 schema"),
        1,
        "the peer message is in the SAME run's second request: {:?}",
        user_texts(&requests[1])
    );
    assert!(
        user_texts(&requests[1])
            .iter()
            .any(|t| t.contains("**From peer**") && t.contains("v2 schema")),
        "as the intercom card the model reads"
    );
    assert_eq!(live.persisted().await.len(), 1, "persisted exactly once");
    assert_eq!(
        live.until_receipt("steer-1", "injected").await,
        ["receiver_received", "acknowledged", "injected"]
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live.script.model_turns(),
        3,
        "no turn was started after the run"
    );
    live.shutdown().await;
}

/// ICOM-068 through the intercom path — `inboundTrigger: "never"` delivers without a turn, and the
/// message is in the model's context on the user's next prompt, ahead of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_trigger_never_reaches_the_model_on_the_next_prompt() {
    let options = Options {
        inbound_trigger: Some("never"),
        ..Options::interactive()
    };
    let live = live(options, answer_every_turn(), no_summaries()).await;

    live.send("quiet-1", "FYI the deploy window moved to 3pm", false)
        .await;
    live.until_receipt("quiet-1", "injected").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while live.persisted().await.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        live.persisted().await.len(),
        1,
        "delivered to the session tree"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(live.script.model_turns(), 0, "without driving a turn");

    live.prompt("next").await;
    live.session.wait_for_idle().await;
    let requests = live.script.requests();
    let texts = user_texts(&requests[0]);
    let note = texts
        .iter()
        .position(|t| t.contains("deploy window moved"))
        .unwrap_or_else(|| panic!("the no-turn message reached the model: {texts:?}"));
    let next = texts.iter().position(|t| t == "next").unwrap();
    assert!(note < next, "{texts:?}");
    live.shutdown().await;
}

/// How a held compaction ends.
#[derive(Clone, Copy, Debug)]
enum Outcome {
    Success,
    Failure,
    Abort,
    Cancel,
}

/// Bring up a live session with two warm-up turns and start a `/compact` that is held open (the
/// summarizer, or for `Cancel` the `session_before_compact` veto, parks on `gate`).
async fn compacting(
    outcome: Outcome,
    gate: &Gate,
) -> (
    Live,
    tokio::task::JoinHandle<Result<cyrup_session_svc::CompactionResult, SessionServiceError>>,
) {
    let summarize: SummaryFn = {
        let gate = gate.clone();
        Arc::new(move || {
            let gate = gate.clone();
            Box::pin(async move {
                gate.passed().await;
                match outcome {
                    Outcome::Failure => faux_assistant_message_with(
                        Vec::new(),
                        StopReason::Error,
                        FauxMessageOptions {
                            error_message: Some("summarizer exploded".into()),
                            ..Default::default()
                        },
                    ),
                    _ => text_reply(SUMMARY),
                }
            })
        })
    };
    let veto = Arc::new(HeldVeto {
        entered: AtomicBool::new(false),
        gate: gate.clone(),
    });
    let options = Options {
        extra: matches!(outcome, Outcome::Cancel).then(|| veto.clone() as Arc<dyn NativeExtension>),
        ..Options::interactive()
    };
    let live = live(options, answer_every_turn(), summarize).await;
    for turn in ["one", "two"] {
        live.prompt(turn).await;
        live.session.wait_for_idle().await;
    }
    assert_eq!(live.script.model_turns(), 2);
    let compaction = {
        let session = live.session.clone();
        tokio::spawn(async move { session.compact(None).await })
    };
    let script = live.script.clone();
    let under_way = within(Duration::from_secs(15), || match outcome {
        Outcome::Cancel => veto.entered.load(Ordering::SeqCst),
        _ => script.summarizer_calls.load(Ordering::SeqCst) > 0,
    })
    .await;
    assert!(under_way, "{outcome:?}: the compaction is held open");
    assert!(live.session.is_compacting());
    assert!(!live.session.is_idle(), "a compaction is busy (SEAM-125)");
    (live, compaction)
}

/// ICOM-062 — upstream's `busy without an agent run holds inbound messages through compaction
/// {success,failure,abort,cancel}` (`intercom.integration.test.ts:2287-2323` @v0.14.0), over a
/// real `/compact`: while it runs both messages are held (`receiver_received, acknowledged,
/// queued`) and no model run starts; once it ends — however it ends — both are delivered with a
/// turn, in order, and the peer is told `injected`.
async fn holds_inbound_through_compaction(outcome: Outcome) {
    let gate = Gate::new();
    let (live, compaction) = compacting(outcome, &gate).await;
    let id1 = format!("{outcome:?}-1");
    let id2 = format!("{outcome:?}-2");
    live.send(&id1, "First held", false).await;
    live.send(&id2, "Second held", false).await;
    live.until_receipt(&id2, "queued").await;
    assert_eq!(
        live.script.receipts(&id1),
        ["receiver_received", "acknowledged", "queued"]
    );
    assert_eq!(
        live.script.receipts(&id2),
        ["receiver_received", "acknowledged", "queued"]
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        live.script.model_turns(),
        2,
        "{outcome:?}: no model run under the compaction"
    );
    // (The tree is not read here: the summarizer call runs under the session-manager lock.)

    // No lifecycle event is needed: the session becomes idle after success, failure, abort or
    // cancellation, and the held messages follow.
    if matches!(outcome, Outcome::Abort) {
        live.session.abort_compaction();
    }
    gate.open();
    let result = compaction.await.unwrap();
    match outcome {
        Outcome::Success => {
            assert!(
                result
                    .expect("the compaction succeeds")
                    .summary
                    .contains(SUMMARY)
            );
        }
        Outcome::Failure | Outcome::Abort | Outcome::Cancel => {
            assert!(
                result.is_err(),
                "{outcome:?}: the compaction does not produce a result"
            );
        }
    }
    assert_eq!(
        live.until_receipt(&id1, "injected").await,
        ["receiver_received", "acknowledged", "queued", "injected"]
    );
    assert_eq!(
        live.until_receipt(&id2, "injected").await,
        ["receiver_received", "acknowledged", "queued", "injected"]
    );
    let script = live.script.clone();
    assert!(
        within(Duration::from_secs(15), || {
            let requests = script.requests();
            requests
                .iter()
                .any(|r| mentions(r, "First held") > 0 && mentions(r, "Second held") > 0)
        })
        .await,
        "{outcome:?}: both held messages reached the model"
    );
    live.session.wait_for_idle().await;

    let requests = live.script.requests();
    let delivered = requests
        .iter()
        .find(|r| mentions(r, "First held") > 0 && mentions(r, "Second held") > 0)
        .unwrap();
    let texts = user_texts(delivered);
    let first = texts.iter().position(|t| t.contains("First held")).unwrap();
    let second = texts
        .iter()
        .position(|t| t.contains("Second held"))
        .unwrap();
    assert!(
        first < second,
        "{outcome:?}: delivered in arrival order: {texts:?}"
    );
    if matches!(outcome, Outcome::Success) {
        assert!(
            texts.iter().any(|t| t.contains(SUMMARY)),
            "the turn runs over the compacted context: {texts:?}"
        );
    }
    let bodies = live.persisted().await;
    assert_eq!(
        bodies.len(),
        2,
        "two messages, each with its own card: {bodies:?}"
    );
    assert!(
        requests[..2].iter().all(|r| mentions(r, "held") == 0),
        "nothing reached a request made before the compaction ended"
    );
    live.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_without_an_agent_run_holds_inbound_messages_through_compaction_success() {
    holds_inbound_through_compaction(Outcome::Success).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_without_an_agent_run_holds_inbound_messages_through_compaction_failure() {
    holds_inbound_through_compaction(Outcome::Failure).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_without_an_agent_run_holds_inbound_messages_through_compaction_abort() {
    holds_inbound_through_compaction(Outcome::Abort).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_without_an_agent_run_holds_inbound_messages_through_compaction_cancel() {
    holds_inbound_through_compaction(Outcome::Cancel).await;
}

/// ICOM-062 — `shutdown discards inbound messages held during compaction` (`:2325-2351`): the
/// session is disposed while the message is held; the sender is told `expired` and it is never
/// injected, even once the compaction is over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_discards_inbound_messages_held_during_compaction() {
    let gate = Gate::new();
    let (mut live, compaction) = compacting(Outcome::Success, &gate).await;
    live.send("held-at-shutdown", "Held", false).await;
    live.until_receipt("held-at-shutdown", "queued").await;

    live.session.dispose("quit").await;
    assert_eq!(
        live.until_receipt("held-at-shutdown", "expired").await,
        ["receiver_received", "acknowledged", "queued", "expired"]
    );
    gate.open();
    let _ = compaction.await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(live.script.model_turns(), 2, "no turn was started for it");
    assert!(live.persisted().await.is_empty(), "never injected");
    assert_eq!(
        live.script.receipts("held-at-shutdown"),
        ["receiver_received", "acknowledged", "queued", "expired"]
    );
    live.peer.disconnect();
    let _ = live.broker.kill().await;
}

/// ICOM-062 — `runtime replacement discards held inbound messages` (`:2381-2401`): a second
/// `session_start` on the extension (a replaced runtime) expires what the old runtime held, so the
/// idle edge after the compaction injects nothing. The event is dispatched to the live session's
/// own extension instance directly: cyrup builds a fresh session — and a fresh extension — for a
/// replacement, so this same-instance restart is only reachable this way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_replacement_discards_held_inbound_messages() {
    let gate = Gate::new();
    let (live, compaction) = compacting(Outcome::Success, &gate).await;
    live.send("old-runtime", "Old runtime", false).await;
    live.until_receipt("old-runtime", "queued").await;

    let ctx = HostCtx::event(cyrup_ext::ExtMode::Tui, true, live.cwd.clone());
    let _ = live
        .ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "reload".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    assert_eq!(
        live.ext.state().held_inbound_len(),
        0,
        "expired by the new runtime"
    );
    gate.open();
    let _ = compaction.await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live.script.model_turns(),
        2,
        "nothing injected into the new runtime"
    );
    assert!(live.persisted().await.is_empty());
    assert!(
        !live
            .script
            .receipts("old-runtime")
            .iter()
            .any(|s| s == "injected")
    );
    live.shutdown().await;
}

/// ICOM-062 — `cancelling a compaction-held message drops it before injection` (`:2353-2379`):
/// the sender's cancel reaches the message while it is still held, so it is dropped with a
/// `cancelled` receipt (not the `cancellation_requested` hedge) and never injected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_compaction_held_message_drops_it_before_injection() {
    let gate = Gate::new();
    let (live, compaction) = compacting(Outcome::Success, &gate).await;
    live.send("held-then-cancelled", "Held", false).await;
    live.until_receipt("held-then-cancelled", "queued").await;
    let cancelled = live
        .peer
        .cancel_message("held-then-cancelled")
        .await
        .expect("the cancel is routed");
    assert!(cancelled.delivered);
    assert_eq!(
        live.until_receipt("held-then-cancelled", "cancelled").await,
        ["receiver_received", "acknowledged", "queued", "cancelled"]
    );
    gate.open();
    compaction.await.unwrap().expect("the compaction succeeds");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live.script.model_turns(),
        2,
        "no turn for a cancelled message"
    );
    assert!(live.persisted().await.is_empty(), "never injected");
    assert_eq!(
        live.script.receipts("held-then-cancelled"),
        ["receiver_received", "acknowledged", "queued", "cancelled"]
    );
    live.shutdown().await;
}

/// Start a run whose first model call fails with a retryable error, and wait until the session
/// sits in the retry backoff: busy (the post-run driver owns the session) WITHOUT an agent run
/// (`agent_end` has been seen) — the same state a compaction leaves the extension in, reachable
/// with a deterministic window.
async fn in_retry_backoff(live: &Live) {
    live.prompt("do the thing").await;
    let session = live.session.clone();
    let state = live.ext.state().clone();
    let script = live.script.clone();
    assert!(
        within(Duration::from_secs(10), || script.model_turns() == 1
            && !state.agent_running()
            && !session.is_idle())
        .await,
        "the session is in the retry backoff: busy without an agent run"
    );
}

fn overloaded() -> AssistantMessage {
    faux_assistant_message_with(
        Vec::new(),
        StopReason::Error,
        FauxMessageOptions {
            error_message: Some("overloaded".into()),
            ..Default::default()
        },
    )
}

/// ICOM-062 — `held inbound messages steer when an agent run is busy after compaction`
/// (`:2403-2421`): a message held while the session is busy without a run is flushed on the NEXT
/// `agent_start` and, the session now being busy WITH a run, steered into it — it reaches that
/// run's model and no extra turn follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_inbound_messages_steer_when_an_agent_run_starts() {
    let slot: Arc<OnceLock<Weak<AgentSession>>> = Arc::new(OnceLock::new());
    let turn: TurnFn = {
        let slot = slot.clone();
        Arc::new(move |n, messages| {
            let slot = slot.clone();
            Box::pin(async move {
                match n {
                    0 => overloaded(),
                    // The retry's first request: wait until the held message has been steered (or
                    // is already in this request), so it lands inside THIS run.
                    1 => {
                        if mentions(&messages, "Steer later") == 0 {
                            let steered = within(Duration::from_secs(15), || {
                                slot.get()
                                    .and_then(Weak::upgrade)
                                    .is_some_and(|s| s.has_queued_messages())
                            })
                            .await;
                            assert!(steered, "the held message was never steered");
                        }
                        read_tool_call()
                    }
                    _ => text_reply("done"),
                }
            })
        })
    };
    let options = Options {
        settings: slow_retry_settings(),
        ..Options::interactive()
    };
    let live = live(options, turn, no_summaries()).await;
    let _ = slot.set(Arc::downgrade(&live.session));

    in_retry_backoff(&live).await;
    live.send("compact-busy-message", "Steer later", false)
        .await;
    live.until_receipt("compact-busy-message", "queued").await;
    assert_eq!(
        live.script.model_turns(),
        1,
        "held, not injected, while busy without a run"
    );

    live.session.wait_for_idle().await;
    assert_eq!(
        live.until_receipt("compact-busy-message", "injected").await,
        ["receiver_received", "acknowledged", "queued", "injected"]
    );
    let requests = live.script.requests();
    assert_eq!(
        requests.len(),
        3,
        "the retry run: two requests, and no extra turn"
    );
    assert_eq!(mentions(&requests[0], "Steer later"), 0);
    assert_eq!(
        mentions(&requests[2], "Steer later"),
        1,
        "steered into the retry run: {:?}",
        user_texts(&requests[2])
    );
    assert_eq!(live.persisted().await.len(), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(live.script.model_turns(), 3);
    live.shutdown().await;
}

/// ICOM-062 — `human-first leaves non-UI sessions on the busy auto-reply path after compaction`
/// (`:2423-2448`), minus the `busyDelivery` knob (ICOM-063, not ported): the hold rule is general,
/// so a NON-interactive session busy without a run holds the peer's ask too; the flush on the next
/// `agent_start` then finds it busy with a run and no UI, and takes the busy auto-reply path —
/// the peer is answered, nothing is injected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_ui_session_holds_then_takes_the_busy_auto_reply_path() {
    let turn: TurnFn = Arc::new(|n, _| {
        ready(match n {
            0 => overloaded(),
            _ => text_reply("done"),
        })
    });
    let options = Options {
        has_ui: false,
        settings: slow_retry_settings(),
        ..Options::interactive()
    };
    let live = live(options, turn, no_summaries()).await;

    in_retry_backoff(&live).await;
    live.send("pipe-ask", "Still there?", true).await;
    live.until_receipt("pipe-ask", "queued").await;

    let script = live.script.clone();
    assert!(
        within(Duration::from_secs(15), || script
            .peer_inbox
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.reply_to.as_deref() == Some("pipe-ask")))
        .await,
        "the peer's ask is answered with the busy auto-reply"
    );
    let reply = live
        .script
        .peer_inbox
        .lock()
        .unwrap()
        .iter()
        .find(|m| m.reply_to.as_deref() == Some("pipe-ask"))
        .cloned()
        .unwrap();
    assert!(
        reply.content.text.contains("non-interactive"),
        "{:?}",
        reply.content.text
    );
    live.session.wait_for_idle().await;
    assert!(live.persisted().await.is_empty(), "nothing was injected");
    assert!(
        live.script
            .requests()
            .iter()
            .all(|r| mentions(r, "Still there?") == 0),
        "the ask never reached the model"
    );
    live.shutdown().await;
}

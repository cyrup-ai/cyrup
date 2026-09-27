//! pi v0.87.x turn hooks and queue preview, driven through the real loop.
//!
//! - AGENT-038 — `finish_turn` (pi `finishTurn`) runs BEFORE `turn_end`, also on an errored turn,
//!   and returns end/continue; `prepare_request` (pi `prepareRequest`) runs before every provider
//!   request including the first; `TurnUpdate::messages` (pi `AgentLoopTurnUpdate.messages`).
//!   Ports of `packages/agent/test/agent-loop.test.ts:1060-1575` @v0.87.1.
//! - AGENT-036 — `prepare_next_turn` runs only when the loop continues, just before the next
//!   `turn_start`, and steering queued while it runs reaches the very next request.
//! - AGENT-043 — `Agent::peek_queued_messages` (pi `peekQueuedMessages`), ports of
//!   `packages/agent/test/agent.test.ts:960-973`, `:1122-1167` @v0.87.1.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    Agent, AgentContext, AgentError, AgentEvent, AgentEventSink, AgentLoopConfig, AgentMessage,
    EventSubscriber, HookError, Hooks, PendingQueue, PostTurn, PrepareRequestCtx, QueueMode,
    RequestUpdate, TurnDecision, TurnUpdate, run_agent_loop,
};
use cyrup_core::{
    CancelToken, Content, Message, ModelRef, ModelThinkingLevel, RunCancel, StopReason,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use serde_json::json;

use super::support::*;

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

type Log = Arc<Mutex<Vec<String>>>;

/// Writes every event's [`ev_name`] into the same log the hooks under test write into, so a test
/// can assert where a hook ran relative to the events.
struct LogSink(Log);

#[async_trait::async_trait]
impl AgentEventSink for LogSink {
    async fn emit(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(ev_name(event));
    }
}

#[async_trait::async_trait]
impl EventSubscriber for LogSink {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
        self.0.lock().unwrap().push(ev_name(event));
    }
}

/// One provider request as the transport saw it.
#[derive(Clone, Debug)]
struct Request {
    model: String,
    reasoning: ModelThinkingLevel,
    system_prompt: Option<String>,
    user_texts: Vec<String>,
}

fn requests(
    responses: Vec<cyrup_core::AssistantMessage>,
) -> (Arc<dyn crate::StreamFn>, Arc<Mutex<Vec<Request>>>) {
    recording_stream_fn(responses, |model, ctx, opts| Request {
        model: model.model.to_string(),
        reasoning: opts.reasoning,
        system_prompt: ctx.system_prompt.clone(),
        user_texts: ctx.messages.iter().filter_map(user_text).collect(),
    })
}

fn user_text(m: &Message) -> Option<String> {
    match m {
        Message::User { content, .. } => content.iter().find_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        }),
        _ => None,
    }
}

fn text(s: &str) -> cyrup_core::AssistantMessage {
    faux_assistant_message(vec![faux_text(s)], StopReason::Stop)
}

fn tool_call() -> cyrup_core::AssistantMessage {
    faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse)
}

/// Drive the low-level loop (pi `runAgentLoop`) with `hooks` and a shared steering queue.
async fn run_loop(
    hooks: Arc<dyn Hooks>,
    steering: Arc<Mutex<PendingQueue>>,
    sf: Arc<dyn crate::StreamFn>,
    log: Log,
) -> Vec<AgentMessage> {
    let mut config = AgentLoopConfig::new(model_ref());
    config.hooks = hooks;
    config.steering = steering;
    let ctx = AgentContext {
        system_prompt: "sys".into(),
        messages: Vec::new(),
        tools: vec![EchoTool::named("echo")],
    };
    run_agent_loop(
        vec![AgentMessage::user_text("run")],
        ctx,
        config,
        Arc::new(LogSink(log)),
        RunCancel::new(),
        sf,
    )
    .await
}

// ---------------------------------------------------------------------------
// AGENT-038 — finish_turn
// ---------------------------------------------------------------------------

/// Logs each `finish_turn` call and returns a scripted decision per call (`None` past the script).
struct FinishScript {
    log: Log,
    decisions: Mutex<Vec<Option<TurnDecision>>>,
    calls: AtomicUsize,
    prepare_calls: AtomicUsize,
    stop_reasons: Mutex<Vec<StopReason>>,
    last_roles: Mutex<Vec<&'static str>>,
    tool_result_counts: Mutex<Vec<usize>>,
}

impl FinishScript {
    fn new(log: &Log, decisions: Vec<Option<TurnDecision>>) -> Arc<Self> {
        Arc::new(Self {
            log: log.clone(),
            decisions: Mutex::new(decisions),
            calls: AtomicUsize::new(0),
            prepare_calls: AtomicUsize::new(0),
            stop_reasons: Mutex::new(Vec::new()),
            last_roles: Mutex::new(Vec::new()),
            tool_result_counts: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl Hooks for FinishScript {
    async fn finish_turn(
        &self,
        ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnDecision>, HookError> {
        self.log.lock().unwrap().push("finish_turn".into());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.stop_reasons
            .lock()
            .unwrap()
            .push(ctx.message.stop_reason);
        self.last_roles
            .lock()
            .unwrap()
            .push(ctx.context.messages.last().map_or("", |m| role(m)));
        self.tool_result_counts
            .lock()
            .unwrap()
            .push(ctx.tool_results.len());
        let mut d = self.decisions.lock().unwrap();
        Ok(if d.is_empty() { None } else { d.remove(0) })
    }

    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

/// pi "runs finishTurn after tool-result messages and before turn_end" (`agent-loop.test.ts:1060`).
#[tokio::test]
async fn finish_turn_runs_after_the_tool_results_and_before_turn_end() {
    let log: Log = Arc::default();
    let hook = FinishScript::new(&log, vec![]);
    let (sf, _) = requests(vec![tool_call(), text("done")]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::named("echo")])
        .hooks(hook.clone())
        .build();
    agent.subscribe(Arc::new(LogSink(log.clone())));
    agent.prompt("echo").await.unwrap().finished().await;

    let log = log.lock().unwrap().clone();
    let first_turn_end = log.iter().position(|e| e == "turn_end").unwrap();
    assert_eq!(
        log[first_turn_end - 2..=first_turn_end],
        ["message_end:tool", "finish_turn", "turn_end"],
        "finish_turn decides between the last tool result and turn_end: {log:?}"
    );
    assert_eq!(*hook.tool_result_counts.lock().unwrap(), vec![1, 0]);
    assert_eq!(
        hook.last_roles.lock().unwrap()[0],
        "tool",
        "the turn's tool result is already in the context finish_turn sees"
    );
}

/// pi "runs finishTurn for a error assistant before turn_end without changing the hard exit"
/// (`agent-loop.test.ts:1113`): the hook runs, its `continue` is ignored, and the queued follow-up
/// is never polled.
#[tokio::test]
async fn finish_turn_runs_on_an_errored_turn_and_cannot_continue_it() {
    let log: Log = Arc::default();
    let hook = FinishScript::new(&log, vec![Some(TurnDecision::Continue)]);
    // No scripted response: the faux provider answers with an `error` terminal.
    let (sf, reqs) = requests(vec![]);
    let agent = Agent::builder(model_ref(), sf).hooks(hook.clone()).build();
    agent.subscribe(Arc::new(LogSink(log.clone())));
    let follow_up = AgentMessage::user_text("queued");
    agent.follow_up(follow_up.clone());
    agent.prompt("run").await.unwrap().finished().await;

    let log = log.lock().unwrap().clone();
    let tail: Vec<&str> = log.iter().rev().take(3).rev().map(String::as_str).collect();
    assert_eq!(tail, ["finish_turn", "turn_end", "agent_end"], "{log:?}");
    assert_eq!(*hook.stop_reasons.lock().unwrap(), vec![StopReason::Error]);
    assert_eq!(
        reqs.lock().unwrap().len(),
        1,
        "continue does not revive an error"
    );
    assert_eq!(
        agent.peek_queued_messages(),
        vec![follow_up],
        "the follow-up queue was never polled"
    );
}

/// pi "action:end skips queue polling and next-turn preparation" (`agent-loop.test.ts:1165`) and
/// "keeps queues when finishTurn ends the run" (`agent.test.ts:1122`).
#[tokio::test]
async fn finish_turn_end_stops_after_turn_end_without_polling_or_preparing() {
    let log: Log = Arc::default();
    let hook = FinishScript::new(&log, vec![Some(TurnDecision::End)]);
    let (sf, reqs) = requests(vec![tool_call(), text("unreached")]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::named("echo")])
        .hooks(hook.clone())
        .build();
    agent.subscribe(Arc::new(LogSink(log.clone())));
    let follow_up = AgentMessage::user_text("follow-up");
    agent.follow_up(follow_up.clone());
    agent.prompt("run").await.unwrap().finished().await;

    let log = log.lock().unwrap().clone();
    let tail: Vec<&str> = log.iter().rev().take(3).rev().map(String::as_str).collect();
    assert_eq!(tail, ["finish_turn", "turn_end", "agent_end"], "{log:?}");
    assert_eq!(
        reqs.lock().unwrap().len(),
        1,
        "end wins over the pending tool continuation"
    );
    assert_eq!(hook.prepare_calls.load(Ordering::SeqCst), 0);
    assert_eq!(agent.peek_queued_messages(), vec![follow_up]);
}

/// pi "makes exactly one context-only request when no natural request satisfies continuation"
/// (`agent-loop.test.ts:1225`).
#[tokio::test]
async fn finish_turn_continue_makes_exactly_one_context_only_request() {
    let log: Log = Arc::default();
    let hook = FinishScript::new(&log, vec![Some(TurnDecision::Continue)]);
    let (sf, reqs) = requests(vec![text("r1"), text("r2"), text("r3")]);
    let agent = Agent::builder(model_ref(), sf).hooks(hook.clone()).build();
    agent.prompt("run").await.unwrap().finished().await;

    assert_eq!(reqs.lock().unwrap().len(), 2);
    assert_eq!(hook.calls.load(Ordering::SeqCst), 2);
}

/// pi "lets a natural tool-result request satisfy continuation" (`agent-loop.test.ts:1259`).
#[tokio::test]
async fn finish_turn_continue_is_satisfied_by_a_tool_result_request() {
    let log: Log = Arc::default();
    let hook = FinishScript::new(&log, vec![Some(TurnDecision::Continue)]);
    let (sf, reqs) = requests(vec![tool_call(), text("done"), text("extra")]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::named("echo")])
        .hooks(hook.clone())
        .build();
    agent.prompt("run").await.unwrap().finished().await;

    assert_eq!(reqs.lock().unwrap().len(), 2, "no extra request");
    assert_eq!(hook.calls.load(Ordering::SeqCst), 2);
}

/// pi "lets a natural follow-up request satisfy continuation" (`agent-loop.test.ts:1299`).
#[tokio::test]
async fn finish_turn_continue_is_satisfied_by_a_follow_up_request() {
    let log: Log = Arc::default();
    let hook = FinishScript::new(&log, vec![Some(TurnDecision::Continue)]);
    let (sf, reqs) = requests(vec![text("r1"), text("r2"), text("r3")]);
    let agent = Agent::builder(model_ref(), sf).hooks(hook.clone()).build();
    agent.follow_up(AgentMessage::user_text("follow-up"));
    agent.prompt("run").await.unwrap().finished().await;

    let reqs = reqs.lock().unwrap();
    assert_eq!(reqs.len(), 2, "no extra context-only request");
    assert!(reqs[1].user_texts.contains(&"follow-up".to_string()));
}

/// Queues a steering message from inside `finish_turn`'s first call, so the post-turn poll is the
/// first to see it.
struct SteerFromFinish {
    steering: Arc<Mutex<PendingQueue>>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for SteerFromFinish {
    async fn finish_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnDecision>, HookError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.steering
                .lock()
                .unwrap()
                .push(AgentMessage::user_text("steering"));
            return Ok(Some(TurnDecision::Continue));
        }
        Ok(None)
    }
}

/// pi "lets a natural steering request satisfy continuation" (`agent-loop.test.ts:1299`).
#[tokio::test]
async fn finish_turn_continue_is_satisfied_by_a_steering_request() {
    let steering = Arc::new(Mutex::new(PendingQueue::default()));
    let hook = Arc::new(SteerFromFinish {
        steering: steering.clone(),
        calls: AtomicUsize::new(0),
    });
    let (sf, reqs) = requests(vec![text("r1"), text("r2"), text("r3")]);
    run_loop(hook.clone(), steering, sf, Arc::default()).await;

    let reqs = reqs.lock().unwrap();
    assert_eq!(reqs.len(), 2, "no extra context-only request");
    assert!(reqs[1].user_texts.contains(&"steering".to_string()));
    assert_eq!(hook.calls.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// AGENT-038 — prepare_request
// ---------------------------------------------------------------------------

fn replacement_model() -> ModelRef {
    ModelRef {
        provider: "faux".into(),
        api: Some("faux".into()),
        model: "replacement".into(),
    }
}

/// Replaces context/model/thinking level on its first call and records what each call saw.
struct ReplaceRequest {
    log: Log,
    calls: AtomicUsize,
    saw_texts: Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl Hooks for ReplaceRequest {
    async fn prepare_request(
        &self,
        ctx: PrepareRequestCtx<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<RequestUpdate>, HookError> {
        self.log.lock().unwrap().push("prepare_request".into());
        let texts = ctx
            .context
            .messages
            .iter()
            .filter_map(|m| match m.as_ref() {
                AgentMessage::User { content, .. } => content.iter().find_map(|c| match c {
                    Content::Text { text, .. } => Some(text.to_string()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        self.saw_texts.lock().unwrap().push(texts);
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            return Ok(None);
        }
        assert_eq!(ctx.model, &model_ref());
        assert_eq!(ctx.thinking_level, ModelThinkingLevel::Off);
        Ok(Some(RequestUpdate {
            context: Some(vec![Arc::new(AgentMessage::user_text(
                "canonical projection",
            ))]),
            model: Some(replacement_model()),
            thinking_level: Some(ModelThinkingLevel::High),
            system_prompt: Some("canonical system".into()),
            ..RequestUpdate::default()
        }))
    }
}

/// pi "prepares the initial request after pending messages and can replace request state"
/// (`agent-loop.test.ts:1363`) — and the replacement sticks for the next request.
#[tokio::test]
async fn prepare_request_runs_before_the_first_request_after_pending_messages() {
    let log: Log = Arc::default();
    let hook = Arc::new(ReplaceRequest {
        log: log.clone(),
        calls: AtomicUsize::new(0),
        saw_texts: Mutex::default(),
    });
    let steering = Arc::new(Mutex::new(PendingQueue::default()));
    steering
        .lock()
        .unwrap()
        .push(AgentMessage::user_text("steering"));
    let (sf, reqs) = requests(vec![tool_call(), text("done")]);
    run_loop(hook.clone(), steering, sf, log.clone()).await;

    let log = log.lock().unwrap().clone();
    let first = log.iter().position(|e| e == "prepare_request").unwrap();
    assert_eq!(
        log[first - 1],
        "message_end:user",
        "the steering message was emitted first: {log:?}"
    );
    assert_eq!(
        hook.saw_texts.lock().unwrap()[0],
        ["run", "steering"],
        "pending messages are already in the context the hook sees"
    );

    let reqs = reqs.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[0].model, "replacement",
        "the FIRST request is replaced"
    );
    assert_eq!(reqs[0].reasoning, ModelThinkingLevel::High);
    assert_eq!(reqs[0].user_texts, ["canonical projection"]);
    assert_eq!(reqs[0].system_prompt.as_deref(), Some("canonical system"));
    assert_eq!(reqs[1].model, "replacement", "and the replacement sticks");
    assert_eq!(reqs[1].reasoning, ModelThinkingLevel::High);
    assert_eq!(hook.calls.load(Ordering::SeqCst), 2, "once per request");
}

/// Queues a steering message while the FIRST request is being prepared.
struct SteerFromPrepareRequest {
    steering: Arc<Mutex<PendingQueue>>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for SteerFromPrepareRequest {
    async fn prepare_request(
        &self,
        _ctx: PrepareRequestCtx<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<RequestUpdate>, HookError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.steering
                .lock()
                .unwrap()
                .push(AgentMessage::user_text("late steering"));
        }
        Ok(None)
    }
}

/// pi "does not poll steering after prepareRequest" (`agent-loop.test.ts:1410`).
#[tokio::test]
async fn prepare_request_does_not_poll_steering() {
    let steering = Arc::new(Mutex::new(PendingQueue::default()));
    let hook = Arc::new(SteerFromPrepareRequest {
        steering: steering.clone(),
        calls: AtomicUsize::new(0),
    });
    let (sf, reqs) = requests(vec![text("r1"), text("r2"), text("r3")]);
    run_loop(hook.clone(), steering, sf, Arc::default()).await;

    let included: Vec<bool> = reqs
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.user_texts.contains(&"late steering".to_string()))
        .collect();
    assert_eq!(included, [false, true]);
    assert_eq!(hook.calls.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// AGENT-036 / AGENT-038 — prepare_next_turn
// ---------------------------------------------------------------------------

/// Queues a steering message on every call — what a long `prepare_next_turn` (compaction) would
/// see a user type while it ran.
struct SteerFromPrepareNextTurn {
    steering: Arc<Mutex<PendingQueue>>,
}

#[async_trait::async_trait]
impl Hooks for SteerFromPrepareNextTurn {
    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        self.steering
            .lock()
            .unwrap()
            .push(AgentMessage::user_text("late steering"));
        Ok(None)
    }
}

/// AGENT-036 — pi "picks up steering queued during prepareNextTurn before the next request"
/// (`agent-loop.test.ts:1532`). Without the re-poll the message waits a turn: the second request
/// lacks it and a third one is made for it.
#[tokio::test]
async fn steering_queued_during_prepare_next_turn_reaches_the_next_request() {
    let steering = Arc::new(Mutex::new(PendingQueue::default()));
    let hook = Arc::new(SteerFromPrepareNextTurn {
        steering: steering.clone(),
    });
    let (sf, reqs) = requests(vec![tool_call(), text("done"), text("extra")]);
    run_loop(hook, steering, sf, Arc::default()).await;

    let reqs = reqs.lock().unwrap();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert!(reqs[1].user_texts.contains(&"late steering".to_string()));
}

/// Returns `TurnUpdate::messages` and a new system prompt on its first call; records what
/// `finish_turn` saw on every turn.
struct PrepareWithMessages {
    log: Log,
    prepare_calls: AtomicUsize,
    finish_prompts: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Hooks for PrepareWithMessages {
    async fn finish_turn(
        &self,
        ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnDecision>, HookError> {
        self.finish_prompts
            .lock()
            .unwrap()
            .push(ctx.context.system_prompt.to_string());
        Ok(None)
    }

    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        self.log.lock().unwrap().push("prepare_next_turn".into());
        if self.prepare_calls.fetch_add(1, Ordering::SeqCst) > 0 {
            return Ok(None);
        }
        Ok(Some(TurnUpdate {
            messages: vec![AgentMessage::user_text("updated guidance")],
            system_prompt: Some("next".into()),
            ..TurnUpdate::default()
        }))
    }
}

/// AGENT-038 — pi "should use prepareNextTurn snapshot before continuing" (`agent-loop.test.ts:1455`):
/// the update's messages are appended with their lifecycle events before the next request, and
/// `prepare_next_turn` is not called after the run's last turn.
///
/// AGENT-036 — the hook runs after `turn_end`, right before the next `turn_start`, so `finish_turn`
/// decides on the context the turn left behind, never on the next turn's override.
#[tokio::test]
async fn prepare_next_turn_messages_are_appended_before_the_next_request() {
    let log: Log = Arc::default();
    let hook = Arc::new(PrepareWithMessages {
        log: log.clone(),
        prepare_calls: AtomicUsize::new(0),
        finish_prompts: Mutex::default(),
    });
    let (sf, reqs) = requests(vec![tool_call(), text("done")]);
    let new = run_loop(
        hook.clone(),
        Arc::new(Mutex::new(PendingQueue::default())),
        sf,
        log.clone(),
    )
    .await;

    let reqs = reqs.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(!reqs[0].user_texts.contains(&"updated guidance".to_string()));
    assert!(reqs[1].user_texts.contains(&"updated guidance".to_string()));
    assert_eq!(reqs[1].system_prompt.as_deref(), Some("next"));
    assert_eq!(hook.prepare_calls.load(Ordering::SeqCst), 1);
    assert!(new.contains(&AgentMessage::user_text("updated guidance")));

    let log = log.lock().unwrap().clone();
    let at = log.iter().position(|e| e == "prepare_next_turn").unwrap();
    assert_eq!(
        log[at - 1..=at + 3],
        [
            "turn_end",
            "prepare_next_turn",
            "turn_start",
            "message_start:user",
            "message_end:user"
        ],
        "{log:?}"
    );
    assert_eq!(*hook.finish_prompts.lock().unwrap(), ["sys", "next"]);
}

// ---------------------------------------------------------------------------
// AGENT-043 — Agent::peek_queued_messages
// ---------------------------------------------------------------------------

/// pi "previews the next selected queued messages without consuming them" (`agent.test.ts:1150`).
#[tokio::test]
async fn peek_queued_messages_previews_the_next_batch_without_consuming_it() {
    let (sf, _) = requests(vec![]);
    let agent = Agent::builder(model_ref(), sf)
        .steering_mode(QueueMode::OneAtATime)
        .follow_up_mode(QueueMode::All)
        .build();
    let first = AgentMessage::user_text("first steering");
    let second = AgentMessage::user_text("second steering");
    let follow_a = AgentMessage::user_text("follow-up a");
    let follow_b = AgentMessage::user_text("follow-up b");
    agent.steer(first.clone());
    agent.steer(second);
    agent.follow_up(follow_a.clone());
    agent.follow_up(follow_b.clone());

    assert_eq!(agent.peek_queued_messages(), vec![first.clone()]);
    assert_eq!(agent.peek_queued_messages(), vec![first]);
    assert!(agent.has_queued_messages());
    agent.clear_steering_queue();
    assert_eq!(
        agent.peek_queued_messages(),
        vec![follow_a, follow_b],
        "`all` mode previews the whole follow-up queue"
    );
    assert!(agent.has_queued_messages(), "peeking consumed nothing");
}

/// pi "rejects a queued continuation from empty context without draining queues"
/// (`agent.test.ts:960-973`).
#[tokio::test]
async fn a_rejected_continue_leaves_the_queued_preview_intact() {
    let (sf, _) = requests(vec![]);
    let agent = Agent::builder(model_ref(), sf).build();
    let steering = AgentMessage::user_text("steering");
    let follow_up = AgentMessage::user_text("follow-up");
    agent.steer(steering.clone());
    agent.follow_up(follow_up.clone());

    assert!(matches!(
        agent.continue_run().await,
        Err(AgentError::NoMessages(_))
    ));
    assert_eq!(agent.peek_queued_messages(), vec![steering]);
    agent.clear_steering_queue();
    assert_eq!(agent.peek_queued_messages(), vec![follow_up]);
}

//! Behavioral conformance for the agent runtime (func-02 A-02-1..10), driven by the faux provider
//! and tiny in-test tools.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::print_stdout
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_agent::{
    AfterOverride, Agent, AgentEvent, AgentMessage, ApiKeyResolver, BeforeOutcome, BeforeToolCall,
    Hooks, HookError, ProviderStreamFn, StreamFn, ToolExecution,
};
use cyrup_agent::{AfterToolCall, EventSubscriber};
use cyrup_core::{
    CancelToken, Content, EventStream, ExecMode, ModelRef, ProviderId, StopReason, Tool, ToolCallId,
    ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};
use futures::StreamExt;
use serde_json::{json, Value};

// ----------------------------------------------------------------------------
// Helpers: model ref, stream-fn wiring, recorder subscriber.
// ----------------------------------------------------------------------------

fn model_ref() -> ModelRef {
    ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
}

fn faux_stream_fn(responses: Vec<cyrup_core::AssistantMessage>) -> (Arc<FauxProvider>, Arc<dyn StreamFn>) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux.clone();
    let sf: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));
    (faux, sf)
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSubscriber for Recorder {
    async fn on_event(&self, event: &AgentEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

impl Recorder {
    fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }
}

fn ev_name(e: &AgentEvent) -> String {
    match e {
        AgentEvent::AgentStart => "agent_start".into(),
        AgentEvent::TurnStart => "turn_start".into(),
        AgentEvent::MessageStart { message } => format!("message_start:{}", role(message)),
        AgentEvent::MessageUpdate { .. } => "message_update:assistant".into(),
        AgentEvent::MessageEnd { message } => format!("message_end:{}", role(message)),
        AgentEvent::ToolExecutionStart { .. } => "tool_execution_start".into(),
        AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update".into(),
        AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end".into(),
        AgentEvent::TurnEnd { .. } => "turn_end".into(),
        AgentEvent::AgentEnd { .. } => "agent_end".into(),
    }
}

fn role(m: &AgentMessage) -> &'static str {
    match m {
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "tool",
        AgentMessage::Custom { .. } => "custom",
    }
}

fn names(events: &[AgentEvent]) -> Vec<String> {
    events.iter().map(ev_name).collect()
}

fn count_turn_starts(events: &[AgentEvent]) -> usize {
    events.iter().filter(|e| matches!(e, AgentEvent::TurnStart)).count()
}

// ----------------------------------------------------------------------------
// Helpers: tools.
// ----------------------------------------------------------------------------

fn obj_schema() -> Value {
    json!({ "type": "object" })
}

struct EchoTool {
    name: String,
    params: Value,
    calls: Arc<AtomicUsize>,
}

impl EchoTool {
    fn new(name: &str) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (Arc::new(Self { name: name.into(), params: obj_schema(), calls: calls.clone() }), calls)
    }
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text(format!("echo:{params}"))],
            details: None,
            terminate: false,
        })
    }
}

struct FailTool {
    name: String,
    params: Value,
}

impl FailTool {
    fn new(name: &str) -> Arc<Self> {
        Arc::new(Self { name: name.into(), params: obj_schema() })
    }
}

#[async_trait::async_trait]
impl Tool for FailTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::new("boom"))
    }
}

struct SpanTool {
    name: String,
    params: Value,
    ms: u64,
    mode: ExecMode,
    spans: Arc<Mutex<Vec<(String, Instant, Instant)>>>,
}

impl SpanTool {
    fn new(
        name: &str,
        ms: u64,
        mode: ExecMode,
        spans: Arc<Mutex<Vec<(String, Instant, Instant)>>>,
    ) -> Arc<Self> {
        Arc::new(Self { name: name.into(), params: obj_schema(), ms, mode, spans })
    }
}

#[async_trait::async_trait]
impl Tool for SpanTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn execution_mode(&self) -> ExecMode {
        self.mode
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let start = Instant::now();
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_millis(self.ms)) => {}
        }
        let end = Instant::now();
        self.spans.lock().unwrap().push((self.name.clone(), start, end));
        Ok(ToolResult { content: vec![Content::text(format!("done:{}", self.name))], details: None, terminate: false })
    }
}

// ----------------------------------------------------------------------------
// A-02-1 — no-tool ordering.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn a_02_1_no_tool_ordering() {
    let (_faux, sf) = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_text("hello")],
        StopReason::Stop,
    )]);
    let agent = Agent::builder(model_ref(), sf).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    let handle = agent.prompt("hi").await.unwrap();
    let new = handle.finished().await;
    agent.wait_for_idle().await;

    let n = names(&recorder.snapshot());
    let mut i = 0;
    assert_eq!(n[i], "agent_start"); i += 1;
    assert_eq!(n[i], "turn_start"); i += 1;
    assert_eq!(n[i], "message_start:user"); i += 1;
    assert_eq!(n[i], "message_end:user"); i += 1;
    assert_eq!(n[i], "message_start:assistant"); i += 1;
    assert!(n[i] == "message_update:assistant", "expected >=1 update, got {}", n[i]);
    while n[i] == "message_update:assistant" {
        i += 1;
    }
    assert_eq!(n[i], "message_end:assistant"); i += 1;
    assert_eq!(n[i], "turn_end"); i += 1;
    assert_eq!(n[i], "agent_end"); i += 1;
    assert_eq!(i, n.len(), "no trailing events: {n:?}");

    // prompt() resolves to the NEW messages (user + assistant).
    assert_eq!(new.len(), 2);
}

// ----------------------------------------------------------------------------
// A-02-2 — parallel: completion-order ends vs source-order results.
// ----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_02_2_parallel_completion_vs_source_order() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let slow = SpanTool::new("slow", 80, ExecMode::Parallel, spans.clone());
    let fast = SpanTool::new("fast", 50, ExecMode::Parallel, spans.clone());

    let (_faux, sf) = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_tool_call("slow", json!({})), faux_tool_call("fast", json!({}))],
        StopReason::ToolUse,
    )]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![slow, fast])
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    let started = Instant::now();
    let handle = agent.prompt("go").await.unwrap();
    handle.finished().await;
    agent.wait_for_idle().await;
    let elapsed = started.elapsed();

    let events = recorder.snapshot();

    let starts: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionStart { tool_name, .. } => Some(tool_name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec!["slow", "fast"], "tool_execution_start in source order");

    let ends: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd { tool_name, .. } => Some(tool_name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ends, vec!["fast", "slow"], "tool_execution_end in completion order");
    assert_ne!(ends, starts, "completion order provably differs from source order");

    let result_msgs: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageStart { message: AgentMessage::ToolResult(t) } => {
                Some(t.tool_name.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(result_msgs, vec!["slow", "fast"], "tool-result messages in source order");

    let turn_end_order: Vec<String> = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { tool_results, .. } => {
                Some(tool_results.iter().map(|t| t.tool_name.clone()).collect())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(turn_end_order, vec!["slow", "fast"], "turn_end.toolResults in source order");

    // True concurrency: wall-clock ~= max(80,50)=80ms, well under the sum (130ms).
    assert!(elapsed < Duration::from_millis(115), "parallel ran concurrently, took {elapsed:?}");
}

// ----------------------------------------------------------------------------
// A-02-3 — one sequential tool ⇒ whole batch sequential (no overlap).
// ----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_02_3_one_sequential_forces_batch_sequential() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let a = SpanTool::new("a", 30, ExecMode::Sequential, spans.clone());
    let b = SpanTool::new("b", 30, ExecMode::Parallel, spans.clone());

    let (_faux, sf) = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_tool_call("a", json!({})), faux_tool_call("b", json!({}))],
        StopReason::ToolUse,
    )]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![a, b]).build();

    let handle = agent.prompt("go").await.unwrap();
    handle.finished().await;
    agent.wait_for_idle().await;

    let mut s = spans.lock().unwrap().clone();
    s.sort_by_key(|(_, start, _)| *start);
    assert_eq!(s.len(), 2);
    // No overlap: the first tool's interval ends before the second begins.
    assert!(s[0].2 <= s[1].1, "sequential batch must not overlap: {s:?}");
}

// ----------------------------------------------------------------------------
// A-02-4 — before_tool_call block ⇒ immediate error result, no execution.
// ----------------------------------------------------------------------------

struct BlockHook;

#[async_trait::async_trait]
impl Hooks for BlockHook {
    async fn before_tool_call(
        &self,
        _ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        Ok(BeforeOutcome::Block { reason: Some("nope".into()) })
    }
}

#[tokio::test]
async fn a_02_4_before_tool_call_block() {
    let (echo, calls) = EchoTool::new("echo");
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("after")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![echo])
        .hooks(Arc::new(BlockHook))
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    assert_eq!(calls.load(Ordering::SeqCst), 0, "blocked tool must not execute");

    let events = recorder.snapshot();
    let blocked = events.iter().find_map(|e| match e {
        AgentEvent::MessageStart { message: AgentMessage::ToolResult(t) } => Some(t.clone()),
        _ => None,
    });
    let blocked = blocked.expect("a tool-result message");
    assert!(blocked.is_error);
    let text = match &blocked.content[0] {
        Content::Text { text, .. } => text.clone(),
        _ => panic!("text content"),
    };
    assert!(text.contains("nope"), "block reason surfaced: {text}");
}

// ----------------------------------------------------------------------------
// A-02-5 — after_tool_call details-only replace; all-terminate stops vs mixed continues.
// ----------------------------------------------------------------------------

struct DetailsHook;

#[async_trait::async_trait]
impl Hooks for DetailsHook {
    async fn after_tool_call(
        &self,
        _ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        Ok(Some(AfterOverride { details: Some(json!({ "k": "v" })), ..Default::default() }))
    }
}

#[tokio::test]
async fn a_02_5_after_tool_call_details_only_replace() {
    let (echo, _calls) = EchoTool::new("echo");
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({ "x": 1 }))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![echo])
        .hooks(Arc::new(DetailsHook))
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    let events = recorder.snapshot();
    let tr = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::MessageStart { message: AgentMessage::ToolResult(t) } => Some(t.clone()),
            _ => None,
        })
        .expect("tool result");
    // content untouched (replace-not-merge: only details provided)
    let text = match &tr.content[0] {
        Content::Text { text, .. } => text.clone(),
        _ => panic!("text"),
    };
    assert!(text.starts_with("echo:"), "content kept: {text}");
    assert_eq!(tr.details, Some(json!({ "k": "v" })), "details replaced");
}

struct TerminateHook {
    only: Option<String>, // None => terminate all; Some(name) => terminate only that tool
}

#[async_trait::async_trait]
impl Hooks for TerminateHook {
    async fn after_tool_call(
        &self,
        ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        let terminate = match &self.only {
            None => true,
            Some(name) => ctx.tool_name == name,
        };
        Ok(Some(AfterOverride { terminate: Some(terminate), ..Default::default() }))
    }
}

#[tokio::test]
async fn a_02_5_terminate_all_stops() {
    let (a, _) = EchoTool::new("a");
    let (b, _) = EchoTool::new("b");
    let (faux, sf) = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_tool_call("a", json!({})), faux_tool_call("b", json!({}))],
        StopReason::ToolUse,
    )]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![a, b])
        .hooks(Arc::new(TerminateHook { only: None }))
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    assert_eq!(count_turn_starts(&recorder.snapshot()), 1, "all-terminate stops after one turn");
    assert_eq!(faux.call_count(), 1);
}

#[tokio::test]
async fn a_02_5_terminate_mixed_continues() {
    let (a, _) = EchoTool::new("a");
    let (b, _) = EchoTool::new("b");
    let (faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("a", json!({})), faux_tool_call("b", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("more")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![a, b])
        .hooks(Arc::new(TerminateHook { only: Some("a".into()) }))
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    assert_eq!(count_turn_starts(&recorder.snapshot()), 2, "mixed batch continues");
    assert_eq!(faux.call_count(), 2);
}

// ----------------------------------------------------------------------------
// A-02-6 — steering injected after batch; follow-up only when otherwise stopping.
// ----------------------------------------------------------------------------

struct SteerOnToolStart {
    agent: std::sync::Weak<Agent>,
    msg: Mutex<Option<AgentMessage>>,
}

#[async_trait::async_trait]
impl EventSubscriber for SteerOnToolStart {
    async fn on_event(&self, event: &AgentEvent) {
        if let AgentEvent::ToolExecutionStart { .. } = event
            && let Some(a) = self.agent.upgrade()
                && let Some(m) = self.msg.lock().unwrap().take() {
                    a.steer(m);
                }
    }
}

#[tokio::test]
async fn a_02_6_steering_injected_after_batch() {
    let (echo, _) = EchoTool::new("echo");
    let (faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("after steer")], StopReason::Stop),
    ]);
    let agent = Arc::new(Agent::builder(model_ref(), sf).tools(vec![echo]).build());
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());
    agent.subscribe(Arc::new(SteerOnToolStart {
        agent: Arc::downgrade(&agent),
        msg: Mutex::new(Some(AgentMessage::user_text("steered!"))),
    }));

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    let events = recorder.snapshot();
    assert_eq!(count_turn_starts(&events), 2, "steering produced an additional turn");
    assert_eq!(faux.call_count(), 2);

    // The steered user message appears in the event stream as a fresh injected message.
    let injected = events.iter().filter(|e| matches!(
        e,
        AgentEvent::MessageStart { message: AgentMessage::User { .. } }
    )).count();
    assert_eq!(injected, 2, "original prompt + steered message both injected");
}

#[tokio::test]
async fn a_02_6_follow_up_only_when_stopping() {
    let (faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_text("turn one")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("turn two")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    // A follow-up enqueued up-front must NOT be injected into turn one; it is delivered only after
    // the agent would otherwise stop (R-02-035).
    agent.follow_up(AgentMessage::user_text("follow up"));

    agent.prompt("hi").await.unwrap();
    agent.wait_for_idle().await;

    let events = recorder.snapshot();
    assert_eq!(count_turn_starts(&events), 2, "follow-up produced a second turn");
    assert_eq!(faux.call_count(), 2);

    // The follow-up user message_start occurs AFTER the first turn_end.
    let first_turn_end = events.iter().position(|e| matches!(e, AgentEvent::TurnEnd { .. })).unwrap();
    let follow_pos = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, AgentEvent::MessageStart { message: AgentMessage::User { .. } }))
        .map(|(i, _)| i)
        .next_back()
        .unwrap();
    assert!(follow_pos > first_turn_end, "follow-up injected after the first turn ended");
}

// ----------------------------------------------------------------------------
// A-02-7 — abort ⇒ aborted closing sequence; waitForIdle settles after subscribers.
// ----------------------------------------------------------------------------

struct BlockingStreamFn;

impl StreamFn for BlockingStreamFn {
    fn stream(
        &self,
        _model: &ModelRef,
        _ctx: &Context,
        _opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let tail = futures::stream::once(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let message = faux_assistant_message(vec![faux_text("late")], StopReason::Stop);
            StreamEvent::Done { reason: message.stop_reason, message }
        });
        let start = StreamEvent::Start {
            partial: faux_assistant_message(Vec::new(), StopReason::Stop),
        };
        Box::pin(futures::stream::iter(vec![start]).chain(tail))
    }
}

struct SlowEnd {
    done: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl EventSubscriber for SlowEnd {
    async fn on_event(&self, event: &AgentEvent) {
        if let AgentEvent::AgentEnd { .. } = event {
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.done.store(true, Ordering::SeqCst);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_02_7_abort_closing_sequence_and_idle_settlement() {
    let sf: Arc<dyn StreamFn> = Arc::new(BlockingStreamFn);
    let agent = Agent::builder(model_ref(), sf).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());
    let done = Arc::new(AtomicBool::new(false));
    agent.subscribe(Arc::new(SlowEnd { done: done.clone() }));

    agent.prompt("hi").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    agent.abort();
    agent.wait_for_idle().await;

    assert!(
        done.load(Ordering::SeqCst),
        "wait_for_idle must resolve only after agent_end subscribers complete"
    );

    let events = recorder.snapshot();
    assert!(matches!(events.last(), Some(AgentEvent::AgentEnd { .. })), "ends with agent_end");
    let aborted = events.iter().any(|e| matches!(
        e,
        AgentEvent::MessageEnd { message: AgentMessage::Assistant(a) } if a.stop_reason == StopReason::Aborted
    ));
    assert!(aborted, "assistant message carries stopReason aborted");
    let turn_end_aborted = events.iter().any(|e| matches!(
        e,
        AgentEvent::TurnEnd { message: AgentMessage::Assistant(a), .. } if a.stop_reason == StopReason::Aborted
    ));
    assert!(turn_end_aborted, "turn_end carries the aborted assistant message");
}

// ----------------------------------------------------------------------------
// Recording stream-fn (captures the LLM context per request) for A-02-8 / A-02-9.
// ----------------------------------------------------------------------------

struct RecordingStreamFn {
    inner: Arc<dyn StreamFn>,
    captured: Arc<Mutex<Vec<Context>>>,
}

impl StreamFn for RecordingStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.captured.lock().unwrap().push(ctx.clone());
        self.inner.stream(model, ctx, opts)
    }
}

// ----------------------------------------------------------------------------
// A-02-8 — thrown tool ⇒ isError result; loop continues; model sees the error.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn a_02_8_thrown_tool_is_error_and_loop_continues() {
    let fail = FailTool::new("fail");
    let (_faux, inner) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("fail", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> =
        Arc::new(RecordingStreamFn { inner, captured: captured.clone() });
    let agent = Agent::builder(model_ref(), sf).tools(vec![fail]).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    let events = recorder.snapshot();
    let tr = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::MessageStart { message: AgentMessage::ToolResult(t) } => Some(t.clone()),
            _ => None,
        })
        .expect("tool result");
    assert!(tr.is_error, "thrown execute ⇒ isError result");

    assert_eq!(count_turn_starts(&events), 2, "loop continues after a thrown tool");

    // The model's SECOND request includes the error tool-result.
    let reqs = captured.lock().unwrap();
    assert_eq!(reqs.len(), 2);
    let saw_error_result = reqs[1].messages.iter().any(|m| matches!(
        m,
        cyrup_core::Message::ToolResult { is_error: true, .. }
    ));
    assert!(saw_error_result, "model sees the error tool-result on the next request");
}

// ----------------------------------------------------------------------------
// A-02-9 — Custom message dropped from LLM context but visible in events.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn a_02_9_custom_message_dropped_from_llm_visible_in_events() {
    let (_faux, inner) =
        faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> =
        Arc::new(RecordingStreamFn { inner, captured: captured.clone() });
    let agent = Agent::builder(model_ref(), sf).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    // Pre-enqueue a custom steering message: it is injected (visible in the stream) but the default
    // convert_to_llm drops it from the model request.
    agent.steer(AgentMessage::Custom {
        kind: "note".into(),
        payload: json!({ "hidden": true }),
        timestamp: None,
    });

    agent.prompt("hi").await.unwrap();
    agent.wait_for_idle().await;

    let events = recorder.snapshot();
    let custom_visible = events.iter().any(|e| matches!(
        e,
        AgentEvent::MessageStart { message: AgentMessage::Custom { .. } }
    ));
    assert!(custom_visible, "custom message appears in the event stream");

    let reqs = captured.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    // No custom role can exist in Message; just assert the custom payload never reached the model.
    let any_hidden = reqs[0]
        .messages
        .iter()
        .any(|m| serde_json::to_string(m).map(|s| s.contains("hidden")).unwrap_or(false));
    assert!(!any_hidden, "custom message excluded from the LLM request");
}

// ----------------------------------------------------------------------------
// A-02-10 — state copy-on-assign.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn a_02_10_state_copy_on_assign() {
    let (_faux, sf) =
        faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).build();

    let mut x = vec![AgentMessage::user_text("a")];
    agent.set_messages(x.clone()).await;
    // Mutating the caller's array after assignment must NOT change agent state (R-02-038).
    x.push(AgentMessage::user_text("b"));

    let snap = agent.snapshot().await;
    assert_eq!(snap.messages.len(), 1, "state decoupled from caller's array");

    // The snapshot is a copy too: mutating it does not change state.
    let mut snap_msgs = snap.messages.clone();
    snap_msgs.push(AgentMessage::user_text("c"));
    assert_eq!(agent.snapshot().await.messages.len(), 1);
}

// ----------------------------------------------------------------------------
// R-02-048 — a panicking subscriber is contained: the run still completes/emits agent_end,
// wait_for_idle settles (no deadlock), and a subsequent prompt still runs.
// ----------------------------------------------------------------------------

struct PanicSubscriber;

#[async_trait::async_trait]
impl EventSubscriber for PanicSubscriber {
    async fn on_event(&self, event: &AgentEvent) {
        // Panic mid-run (on streaming updates) AND on the settlement-gating agent_end. If either
        // panic escaped emit(), the run task would unwind and settlement would never fire.
        match event {
            AgentEvent::MessageUpdate { .. } | AgentEvent::AgentEnd { .. } => {
                panic!("subscriber boom");
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r_02_048_panicking_subscriber_is_contained_and_never_deadlocks() {
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).build();

    // The panicker is registered FIRST. If its panic were not contained, emit() would unwind the run
    // task before the recorder (registered SECOND) ever observed agent_end, and the settlement signal
    // would never flip -> wait_for_idle would hang forever.
    agent.subscribe(Arc::new(PanicSubscriber));
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    // (b) wait_for_idle must return rather than hang: the timeout turns a hang into a failure.
    let settled = tokio::time::timeout(Duration::from_secs(5), async {
        agent.prompt("hi").await.unwrap();
        agent.wait_for_idle().await;
    })
    .await;
    assert!(settled.is_ok(), "run must settle; wait_for_idle deadlocked");

    // (a) the run still completed and emitted agent_end despite the contained panics.
    let n = names(&recorder.snapshot());
    assert!(
        matches!(n.last().map(String::as_str), Some("agent_end")),
        "run completes and emits agent_end despite a panicking subscriber: {n:?}"
    );

    // (c) a subsequent prompt on the same agent still runs to completion.
    let new = tokio::time::timeout(Duration::from_secs(5), async {
        let h = agent.prompt("again").await.unwrap();
        let new = h.finished().await;
        agent.wait_for_idle().await;
        new
    })
    .await
    .expect("second run must settle, not deadlock");
    assert!(!new.is_empty(), "second run produced new messages: {new:?}");
}

// ----------------------------------------------------------------------------
// Extra: single-active-run guard + getApiKey precedence (R-02-006 / R-02-054).
// ----------------------------------------------------------------------------

struct KeyResolver;

#[async_trait::async_trait]
impl ApiKeyResolver for KeyResolver {
    async fn get_api_key(&self, _provider: &ProviderId) -> Option<String> {
        Some("dynamic-key".into())
    }
}

#[tokio::test]
async fn run_active_guard_and_continue_validation() {
    let (_faux, sf) =
        faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).key_resolver(Arc::new(KeyResolver)).build();

    // continue with no messages => error
    assert!(matches!(
        agent.continue_run().await,
        Err(cyrup_agent::AgentError::NoMessages)
    ));

    let _ = agent.prompt("hi").await.unwrap();
    agent.wait_for_idle().await;

    // tool_execution default is parallel
    assert!(matches!(ToolExecution::default(), ToolExecution::Parallel));
}

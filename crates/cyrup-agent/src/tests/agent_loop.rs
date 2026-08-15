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

use crate::{
    AfterOverride, Agent, AgentEvent, AgentMessage, ApiKeyResolver, BeforeOutcome, BeforeToolCall,
    Hooks, HookError, ProviderStreamFn, StreamFn, ToolExecution,
};
use crate::{AfterToolCall, EventSubscriber};
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
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
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
            ..Default::default()
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
        Ok(ToolResult { content: vec![Content::text(format!("done:{}", self.name))], details: None, terminate: false, ..Default::default() })
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
//
// AGENT-019 / DRIFT-039 — this test used to assert `elapsed < 115ms` over a span that includes
// faux-provider streaming and every subscriber await, and to derive the expected completion order
// from an 80ms-vs-50ms sleep race. Both are scheduler assertions: under load or a debug-profile
// runner they fail while the code is correct, and the reflex remedy (raise the constant) makes them
// prove nothing. The property actually under test is `await Promise.all(...)`
// (`packages/agent/src/agent-loop.ts:540-542` @v0.83.0) — a STRUCTURAL claim that the batch is
// concurrent — so it is now pinned by overlapping intervals plus a test-driven rendezvous, the dual
// of `a_02_3`'s non-overlap check. No sleep, no wall-clock bound, no scheduler dependency.
// ----------------------------------------------------------------------------

/// A tool that (1) records its own `(name, start, end)` interval, (2) rendezvouses with its sibling
/// on a shared [`tokio::sync::Barrier`] so BOTH bodies are provably in flight at the same instant,
/// and (3) optionally waits for a release signal before returning, which makes completion order a
/// fact rather than a race.
struct OverlapTool {
    name: String,
    params: Value,
    mode: ExecMode,
    spans: Arc<Mutex<Vec<(String, Instant, Instant)>>>,
    barrier: Arc<tokio::sync::Barrier>,
    /// `Some` for the tool that must finish LAST: it parks here until the test releases it.
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl OverlapTool {
    fn new(
        name: &str,
        mode: ExecMode,
        spans: Arc<Mutex<Vec<(String, Instant, Instant)>>>,
        barrier: Arc<tokio::sync::Barrier>,
        release: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            params: obj_schema(),
            mode,
            spans,
            barrier,
            release: Mutex::new(release),
        })
    }
}

#[async_trait::async_trait]
impl Tool for OverlapTool {
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
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let start = Instant::now();
        // Neither body can pass this point until the other has entered its own body, so the two
        // intervals provably overlap. If the loop serialized the batch this would deadlock, which
        // the test's outer timeout turns into a failure.
        self.barrier.wait().await;
        let rx = self.release.lock().unwrap().take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
        let end = Instant::now();
        self.spans.lock().unwrap().push((self.name.clone(), start, end));
        Ok(ToolResult {
            content: vec![Content::text(format!("done:{}", self.name))],
            details: None,
            terminate: false,
            ..Default::default()
        })
    }
}

/// Releases the slow tool the moment the FAST tool's `tool_execution_end` has been emitted, so
/// "fast completed before slow" is an observed ordering rather than a sleep race.
struct ReleaseOnEnd {
    after: String,
    tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait::async_trait]
impl EventSubscriber for ReleaseOnEnd {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
        if let AgentEvent::ToolExecutionEnd { tool_name, .. } = event
            && *tool_name == self.after
            && let Some(tx) = self.tx.lock().unwrap().take()
        {
            let _ = tx.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_02_2_parallel_completion_vs_source_order() {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    // Source order is `slow`, `fast`; `slow` parks until `fast`'s end event has been emitted.
    let slow = OverlapTool::new(
        "slow",
        ExecMode::Parallel,
        spans.clone(),
        barrier.clone(),
        Some(release_rx),
    );
    let fast = OverlapTool::new("fast", ExecMode::Parallel, spans.clone(), barrier, None);

    let (_faux, sf) = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_tool_call("slow", json!({})), faux_tool_call("fast", json!({}))],
        StopReason::ToolUse,
    )]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![slow, fast]).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(Arc::new(ReleaseOnEnd {
        after: "fast".to_string(),
        tx: Mutex::new(Some(release_tx)),
    }));
    agent.subscribe(recorder.clone());

    // A generous ceiling: this is a HANG detector (a serialized batch deadlocks on the barrier),
    // not a latency assertion.
    tokio::time::timeout(Duration::from_secs(10), async {
        let handle = agent.prompt("go").await.unwrap();
        handle.finished().await;
        agent.wait_for_idle().await;
    })
    .await
    .expect("a concurrent batch settles; a serialized one deadlocks on the rendezvous barrier");

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

    // TRUE concurrency, asserted structurally: sorted by start, the first interval must still be
    // open when the second begins. This is the dual of `a_02_3`'s `s[0].2 <= s[1].1` non-overlap
    // check and holds regardless of machine speed or scheduler pressure.
    let mut s = spans.lock().unwrap().clone();
    s.sort_by_key(|(_, start, _)| *start);
    assert_eq!(s.len(), 2, "both tool bodies ran: {s:?}");
    assert!(s[0].2 > s[1].1, "parallel batch must OVERLAP: {s:?}");
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
        Ok(BeforeOutcome::Block { reason: Some("nope".into()), terminate: false })
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
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
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
            StreamEvent::terminal(message)
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
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
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
// AGENT-033 / R-02-048 — a FAILING subscriber fails the RUN, and settlement still cannot deadlock.
//
// pi's `processEvents` awaits each listener bare inside `runWithLifecycle`'s try
// (`packages/agent/src/agent.ts:573-575` and `:487-490` @v0.83.0), so a throwing listener stops the
// listener loop, unwinds, and produces the full `handleRunFailure` quartet with the listener's own
// message as `errorMessage` (`:496-512`). cyrup used to `catch_unwind` and DISCARD, so a broken
// observer was invisible. The half of R-02-048 that is load-bearing — `wait_for_idle()` can NEVER
// deadlock — is carried by `SettlementGuard`, not by swallowing the panic, and is still asserted
// here.
// ----------------------------------------------------------------------------

struct PanicSubscriber;

#[async_trait::async_trait]
impl EventSubscriber for PanicSubscriber {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
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
async fn agent033_panicking_subscriber_fails_the_run_and_never_deadlocks() {
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).build();

    // The recorder is registered FIRST so it still observes the events the panicker aborts on: pi
    // stops iterating the listener set at the throw, so a listener registered AFTER the thrower
    // never sees that event — same here.
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());
    agent.subscribe(Arc::new(PanicSubscriber));

    // (a) settlement still fires; the timeout turns a hang into a failure.
    let settled = tokio::time::timeout(Duration::from_secs(5), async {
        agent.prompt("hi").await.unwrap();
        agent.wait_for_idle().await;
    })
    .await;
    assert!(settled.is_ok(), "run must settle; wait_for_idle deadlocked");

    // (b) the run closed through `handleRunFailure`: a synthetic errored assistant carrying the
    // panic's own text, then the four-event quartet.
    let events = recorder.snapshot();
    let failure = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { message: AgentMessage::Assistant(a), .. } => Some(a.clone()),
            _ => None,
        })
        .expect("a turn_end carrying the failure assistant");
    assert_eq!(failure.stop_reason, StopReason::Error, "not cancelled ⇒ `error`, not `aborted`");
    assert_eq!(
        failure.error_message.as_deref(),
        Some("subscriber boom"),
        "pi reports the thrown listener's own message as `errorMessage` (agent.ts:505)"
    );
    let n = names(&events);
    assert!(
        n.iter().any(|e| e == "turn_end"),
        "the closing quartet reached the surviving listener: {n:?}"
    );

    // (c) the agent is reusable: the failure is a RUN failure, not an agent failure.
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
    // AGENT-034 — pi's `Agent.continue()` throws the literal `"No messages to continue from"`
    // (`packages/agent/src/agent.ts:357` @v0.83.0, `:368` @v0.84.1), which is NOT the string the
    // low-level `agentLoopContinue` uses for the same condition.
    let no_msgs = agent.continue_run().await;
    assert!(matches!(no_msgs, Err(crate::AgentError::NoMessages(crate::ContinueSurface::Agent))));
    assert_eq!(
        no_msgs.err().map(|e| e.to_string()).unwrap_or_default(),
        "No messages to continue from"
    );

    let _ = agent.prompt("hi").await.unwrap();
    agent.wait_for_idle().await;

    // tool_execution default is parallel
    assert!(matches!(ToolExecution::default(), ToolExecution::Parallel));
}

// ----------------------------------------------------------------------------
// AGENT-001 — a `length` stop reason fails the whole tool batch as truncated
// instead of executing it (Pi `failToolCallsFromTruncatedMessage`,
// agent-loop.ts:207-216,374-405).
// ----------------------------------------------------------------------------

/// A tool that records whether `execute` was ever entered.
struct TripwireTool {
    name: String,
    params: Value,
    executed: Arc<AtomicBool>,
}

impl TripwireTool {
    fn new(name: &str) -> (Arc<Self>, Arc<AtomicBool>) {
        let executed = Arc::new(AtomicBool::new(false));
        (
            Arc::new(Self { name: name.into(), params: obj_schema(), executed: executed.clone() }),
            executed,
        )
    }
}

#[async_trait::async_trait]
impl Tool for TripwireTool {
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
        self.executed.store(true, Ordering::SeqCst);
        Ok(ToolResult { content: vec![Content::text("ran")], details: None, terminate: false, ..Default::default() })
    }
}

#[tokio::test]
async fn agent_001_length_stop_fails_tool_batch_without_executing() {
    let (tool, executed) = TripwireTool::new("danger");
    let (_faux, sf) = faux_stream_fn(vec![
        // Turn 1: truncated by the output token limit, but still carrying a tool call.
        faux_assistant_message(
            vec![faux_tool_call("danger", json!({ "command": "rm -rf /tmp/build" }))],
            StopReason::Length,
        ),
        // Turn 2: the model gets its chance to re-issue; end cleanly.
        faux_assistant_message(vec![faux_text("re-issued")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    let handle = agent.prompt("go").await.unwrap();
    handle.finished().await;
    agent.wait_for_idle().await;

    // 1. The tool was NEVER executed.
    assert!(!executed.load(Ordering::SeqCst), "truncated tool call must not be executed");

    let events = recorder.snapshot();

    // 2. Exactly one tool_execution_start / tool_execution_end pair, the end flagged isError.
    let starts: Vec<&AgentEvent> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .collect();
    assert_eq!(starts.len(), 1, "exactly one tool_execution_start: {:?}", names(&events));
    let ends: Vec<&AgentEvent> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        .collect();
    assert_eq!(ends.len(), 1, "exactly one tool_execution_end: {:?}", names(&events));
    match ends[0] {
        AgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
            assert_eq!(tool_name, "danger");
            assert!(*is_error, "truncated batch end must be isError");
        }
        other => panic!("expected tool_execution_end, got {other:?}"),
    }

    // 3. Pi's event order for the batch: start -> end -> message_start/end for the tool result.
    let n = names(&events);
    let si = n.iter().position(|x| x == "tool_execution_start").unwrap();
    assert_eq!(
        &n[si..si + 4],
        &[
            "tool_execution_start".to_string(),
            "tool_execution_end".to_string(),
            "message_start:tool".to_string(),
            "message_end:tool".to_string(),
        ],
        "truncated-batch event order: {n:?}"
    );

    // 4. The tool-result message carries Pi's exact text, byte for byte.
    let expected = "Tool call \"danger\" was not executed: the response hit the output token \
                    limit, so its arguments may be truncated. Re-issue the tool call with \
                    complete arguments.";
    let result_msg = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::MessageEnd { message: AgentMessage::ToolResult(m) } => Some(m.clone()),
            _ => None,
        })
        .expect("a tool-result message_end");
    assert!(result_msg.is_error, "tool result must be isError");
    let text = match result_msg.content.first() {
        Some(Content::Text { text, .. }) => text.clone(),
        other => panic!("expected text content, got {other:?}"),
    };
    assert_eq!(text, expected, "must match Pi's message byte-for-byte");

    // 5. The batch did NOT terminate — the loop ran a second turn so the model can re-issue.
    assert_eq!(count_turn_starts(&events), 2, "loop must continue after a truncated batch: {n:?}");
    assert_eq!(n.last().map(String::as_str), Some("agent_end"));
}

// ----------------------------------------------------------------------------
// AGENT-002 — a parallel batch defers the START of every execution until every
// call in the batch has been prepared. Pi's `executeToolCallsParallel` pushes a
// LAZY closure per prepared call during the prep loop (agent-loop.ts:522-533) and
// only invokes them in the following `Promise.all` (agent-loop.ts:540-542), so no
// tool body runs while a later call's `before_tool_call` — the permission dialog —
// is still open. Deferring the start must NOT serialize the batch: once started,
// the calls still run concurrently.
// ----------------------------------------------------------------------------

struct RendezvousTool {
    name: String,
    params: Value,
    log: Arc<Mutex<Vec<String>>>,
    rendezvous: Arc<tokio::sync::Barrier>,
}

impl RendezvousTool {
    fn new(
        name: &str,
        log: Arc<Mutex<Vec<String>>>,
        rendezvous: Arc<tokio::sync::Barrier>,
    ) -> Arc<Self> {
        Arc::new(Self { name: name.into(), params: obj_schema(), log, rendezvous })
    }
}

#[async_trait::async_trait]
impl Tool for RendezvousTool {
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
        self.log.lock().unwrap().push(format!("exec_start:{}", self.name));
        // Both bodies must be in flight at the same time for this to release.
        if tokio::time::timeout(Duration::from_secs(5), self.rendezvous.wait()).await.is_ok() {
            self.log.lock().unwrap().push(format!("rendezvous:{}", self.name));
        }
        Ok(ToolResult { content: vec![Content::text("ok")], details: None, terminate: false, ..Default::default() })
    }
}

/// Stands in for the permission gate: `before_tool_call` for `gate` blocks (as a human
/// would) while the rest of the batch is still being prepared.
struct SlowGateHook {
    log: Arc<Mutex<Vec<String>>>,
    gate: String,
    delay: Duration,
}

#[async_trait::async_trait]
impl Hooks for SlowGateHook {
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        let name = ctx.tool_name.to_string();
        self.log.lock().unwrap().push(format!("hook_enter:{name}"));
        if name == self.gate {
            tokio::time::sleep(self.delay).await;
        }
        self.log.lock().unwrap().push(format!("hook_exit:{name}"));
        Ok(BeforeOutcome::Proceed)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_002_parallel_defers_execution_until_whole_batch_is_prepared() {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let rendezvous = Arc::new(tokio::sync::Barrier::new(2));
    let first = RendezvousTool::new("first", log.clone(), rendezvous.clone());
    let second = RendezvousTool::new("second", log.clone(), rendezvous.clone());

    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("first", json!({})), faux_tool_call("second", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![first, second])
        .hooks(Arc::new(SlowGateHook {
            log: log.clone(),
            gate: "second".into(),
            delay: Duration::from_millis(250),
        }))
        .build();

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    let seen = log.lock().unwrap().clone();
    let pos = |needle: &str| {
        seen.iter().position(|s| s == needle).unwrap_or_else(|| panic!("missing {needle}: {seen:?}"))
    };

    // The gate for call #2 must close BEFORE call #1's body starts.
    assert!(
        pos("hook_exit:second") < pos("exec_start:first"),
        "tool 'first' executed while 'second' was still in before_tool_call: {seen:?}"
    );
    assert!(
        pos("hook_exit:second") < pos("exec_start:second"),
        "execution must start only after the whole batch is prepared: {seen:?}"
    );

    // …and deferring the start must not serialize the batch: both bodies overlap.
    assert!(
        seen.contains(&"rendezvous:first".to_string())
            && seen.contains(&"rendezvous:second".to_string()),
        "parallel batch must still run concurrently once started: {seen:?}"
    );
}

// ----------------------------------------------------------------------------
// AGENT-020 — a REJECTED `continue_run` must leave the steering / follow-up queue intact.
//
// pi's `continue()` throws its run-active guard as its FIRST statement
// (`packages/agent/src/agent.ts:351-353` @v0.83.0), ahead of `steeringQueue.drain()` at `:361` and
// `followUpQueue.drain()` at `:367`, so a refused continuation leaves both queues untouched and the
// message is still delivered at the loop's next drain point (`agent-loop.ts:259`/`:263`). cyrup
// drained FIRST and only then claimed the latch in `start_run`, dropping the drained
// `Vec<AgentMessage>` on the floor on `Err(RunActive)` — no error, no log, no retry.
//
// The live-terminal repro of 2026-08-13 refuted the filed *Impact* (typing during a stream queues
// and re-drives; it never enters this window — 5/5 delivered) but not the defect: on the AGENT API
// itself the loss is DETERMINISTIC, not a race. Any embedder that calls `continue_run()` while a run
// is in flight — exactly what this test does — loses the queued message outright. That is what these
// two tests pin.
// ----------------------------------------------------------------------------

/// A tool that parks until released, so the run is provably in flight while the transcript already
/// ends with the assistant `tool_use` message.
struct GateTool {
    params: Value,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Tool for GateTool {
    fn name(&self) -> &str {
        "gate"
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
        self.entered.notify_one();
        self.release.notified().await;
        Ok(ToolResult {
            content: vec![Content::text("gated")],
            details: None,
            terminate: false,
            ..Default::default()
        })
    }
}

fn gate_tool() -> (Arc<GateTool>, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    (
        Arc::new(GateTool {
            params: obj_schema(),
            entered: entered.clone(),
            release: release.clone(),
        }),
        entered,
        release,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent020_rejected_continue_keeps_the_steering_message() {
    let (gate, entered, release) = gate_tool();
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("gate", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![gate]).build();

    let handle = agent.prompt("go").await.unwrap();
    // The tool has started ⇒ the run holds the latch AND `state.messages` already ends with the
    // assistant `tool_use` message, so `continue_run` takes its `last_is_assistant` branch.
    entered.notified().await;
    assert!(agent.is_running(), "the run is in flight");

    agent.steer(AgentMessage::user_text("keep-me"));
    let rejected = agent.continue_run().await;
    assert!(
        matches!(rejected, Err(crate::AgentError::RunActive(crate::BusyEntry::Continue))),
        "a continuation during an active run is refused, as pi throws at agent.ts:351-353"
    );
    // AGENT-034 — and it is `continue()`'s OWN message, not the latch's or `prompt()`'s.
    assert_eq!(
        rejected.err().map(|e| e.to_string()).unwrap_or_default(),
        "Agent is already processing. Wait for completion before continuing."
    );
    // RED before the fix: `continue_run` had already drained the queue by the time `start_run`
    // rejected it, so the message was gone.
    assert!(
        agent.has_queued_messages(),
        "a REFUSED continuation must leave the steering queue intact (pi guards before draining)"
    );

    release.notify_one();
    let _ = handle.finished().await;
    agent.wait_for_idle().await;
    // ...and because it survived, the running loop still delivers it.
    assert!(!agent.has_queued_messages(), "the surviving message was delivered by the loop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent020_rejected_continue_keeps_the_follow_up_message() {
    let (gate, entered, release) = gate_tool();
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("gate", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![gate]).build();

    let handle = agent.prompt("go").await.unwrap();
    entered.notified().await;

    // Steering empty, follow-up populated ⇒ `continue_run` reaches its SECOND drain
    // (pi `followUpQueue.drain()`, agent.ts:367).
    agent.follow_up(AgentMessage::user_text("keep-me-too"));
    let rejected = agent.continue_run().await;
    assert!(matches!(rejected, Err(crate::AgentError::RunActive(crate::BusyEntry::Continue))));
    assert!(
        agent.has_queued_messages(),
        "a REFUSED continuation must leave the follow-up queue intact too"
    );

    release.notify_one();
    let _ = handle.finished().await;
    agent.wait_for_idle().await;
}

// ----------------------------------------------------------------------------
// AGENT-016 — a panicking tool must not swallow its slot in EITHER batch mode.
//
// pi wraps every execute in try/catch/finally and converts a throw into
// `{ result: createErrorToolResult(...), isError: true }` (`agent-loop.ts:700-703` @v0.83.0, inside
// `executePreparedToolCall` at `:666-707`) — identically in the parallel and the sequential batch.
// cyrup's PARALLEL path awaited the tool inside a `joinset.spawn` with no `catch_unwind`: on unwind
// the finish message was never sent, `remaining` never reached zero, the drain exited via
// `None => break`, the slot stayed `None` and NO tool-result message was emitted for that call — so
// the next provider request carried an assistant `tool_use` with no matching `tool_result`.
// Unwind builds only (`[profile.release] panic = "abort"` aborts instead), which is every
// `cargo test` / `cargo run` and any embedder that does not opt into abort.
// ----------------------------------------------------------------------------

struct PanicTool {
    name: String,
    params: Value,
}

impl PanicTool {
    fn new(name: &str) -> Arc<Self> {
        Arc::new(Self { name: name.into(), params: obj_schema() })
    }
}

#[async_trait::async_trait]
impl Tool for PanicTool {
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
        panic!("boom-42");
    }
}

async fn agent016_batch(execution: ToolExecution) {
    let (echo, _) = EchoTool::new("ok");
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("boom", json!({})), faux_tool_call("ok", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("after")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![PanicTool::new("boom"), echo])
        .tool_execution(execution)
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    let events = recorder.snapshot();
    let ends: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd { tool_name, result, is_error, .. } => {
                Some((tool_name.clone(), result.clone(), *is_error))
            }
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 2, "both calls in the batch report a tool_execution_end: {ends:?}");
    let (_, boom_result, boom_is_error) =
        ends.iter().find(|(n, _, _)| n == "boom").expect("the panicking call reported an end");
    assert!(*boom_is_error, "a panicking tool is an ERROR result, as pi's catch produces");
    assert!(
        boom_result.to_string().contains("boom-42"),
        "the panic payload becomes the error text (pi surfaces the thrown message): {boom_result}"
    );

    // The invariant that actually breaks the conversation: one tool_result message per tool_use.
    let tool_result_ends = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::MessageEnd { message: AgentMessage::ToolResult(_) }))
        .count();
    assert_eq!(tool_result_ends, 2, "two tool_use blocks ⇒ two tool_result messages");
    let turn_end_results = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { tool_results, .. } => Some(tool_results.len()),
            _ => None,
        })
        .expect("a turn_end was emitted");
    assert_eq!(turn_end_results, 2, "turn_end carries both results");

    // And the pending set is drained, so the next request is well-formed.
    assert!(
        agent.snapshot().await.pending_tool_calls.is_empty(),
        "no tool call is left pending after the batch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent016_panicking_tool_keeps_its_slot_in_a_parallel_batch() {
    agent016_batch(ToolExecution::Parallel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent016_panicking_tool_keeps_its_slot_in_a_sequential_batch() {
    agent016_batch(ToolExecution::Sequential).await;
}

// ----------------------------------------------------------------------------
// PROV-011 — a tool's `constrainedSampling` declaration reaches `Context.tools`.
// ----------------------------------------------------------------------------

/// A tool that OPTS IN, plus one that stays silent, so the assertion below can tell "forwarded"
/// from "stamped onto everything".
struct DeclaringTool {
    name: String,
    params: Value,
    declared: Option<cyrup_core::ConstrainedSampling>,
}

impl DeclaringTool {
    fn new(name: &str, declared: Option<cyrup_core::ConstrainedSampling>) -> Arc<Self> {
        Arc::new(Self { name: name.into(), params: obj_schema(), declared })
    }
}

#[async_trait::async_trait]
impl Tool for DeclaringTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        self.declared.as_ref()
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

/// PROV-011 — pi copies `constrainedSampling` off the `ToolDefinition` onto the runtime `AgentTool`
/// (`packages/coding-agent/src/core/tools/tool-definition-wrapper.ts:14` @v0.83.0) and the loop
/// hands those same tools to the stream verbatim (`tools: context.tools`,
/// `packages/agent/src/agent-loop.ts:301` @v0.83.0). So a declaration made on the tool MUST be
/// visible on the `Context.tools` entry the provider receives.
///
/// The loop used to hardcode `constrained_sampling: None` here, which made the entire opt-in path
/// unreachable: `cyrup_provider::utils::constrained_sampling` could never see a config, and a
/// `strict: "require"` declaration — which upstream FAILS the request when the model cannot honor
/// it — silently degraded to an ordinary unconstrained tool call.
#[tokio::test]
async fn prov011_a_tools_constrained_sampling_declaration_reaches_the_provider() {
    use cyrup_core::{ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants};

    let declared = ConstrainedSampling::Config(ConstrainedSamplingConfig::Grammar {
        variants: GrammarVariants {
            openai_lark: Some("start: /[a-z]+/".into()),
            openai_regex: None,
        },
    });
    let opting = DeclaringTool::new("opting", Some(declared.clone()));
    // pi's explicit opt-OUT literal, which must survive as `false` rather than collapse to absent —
    // `constrainedSampling: false` round-trips through `wrapToolDefinition` unchanged.
    let opting_out = DeclaringTool::new("opting_out", Some(ConstrainedSampling::Disabled(false)));
    // A tool that declares nothing: its entry must stay absent, or "forwarding" would be
    // indistinguishable from stamping a constant onto every tool.
    let silent = DeclaringTool::new("silent", None);

    let (_faux, inner) =
        faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(RecordingStreamFn { inner, captured: captured.clone() });
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![opting, opting_out, silent])
        .build();

    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;

    let reqs = captured.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let tools = &reqs[0].tools;
    // Presence before absence: if the tool set never made it into the request at all, the
    // `is_none()` assertion below would pass vacuously.
    assert_eq!(tools.len(), 3, "all three tools reached the request");
    let find = |n: &str| {
        tools.iter().find(|t| t.name == n).unwrap_or_else(|| panic!("tool {n} in Context.tools"))
    };

    assert_eq!(
        find("opting").constrained_sampling.as_ref(),
        Some(&declared),
        "the grammar declaration reached Context.tools verbatim"
    );
    assert_eq!(
        find("opting_out").constrained_sampling.as_ref(),
        Some(&ConstrainedSampling::Disabled(false)),
        "pi's `false` literal survives as `false`, not as absent"
    );
    assert!(
        find("silent").constrained_sampling.is_none(),
        "a tool that declares nothing keeps the field absent"
    );
}

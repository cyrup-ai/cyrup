//! AGENT-004 / AGENT-005 — the widened tool-result model: `usage` and `added_tool_names` on
//! `cyrup_core::ToolResult`, threaded through `after_tool_call`, the emitted events, the transcript
//! `ToolResultMessage` and the LLM request payload.
//!
//! Pi provenance: `AgentToolResult.usage` / `.addedToolNames` (agent/src/types.ts:360-363),
//! `AfterToolCallResult.usage` (types.ts:83-84), `ToolResultMessage.usage` / `.addedToolNames`
//! (ai/src/types.ts:421-428), `finalizeExecutedToolCall` (agent-loop.ts:736-742) and
//! `createToolResultMessage` (agent-loop.ts:773-787).
//!
//! Every assertion here is on OBSERVABLE output: the event stream a subscriber sees, and the
//! `Context.messages` the provider is handed on the following turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::print_stdout
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    AfterOverride, AfterToolCall, Agent, AgentEvent, AgentMessage, EventSubscriber, HookError,
    Hooks, ProviderStreamFn, StreamFn, ToolResultMessage,
};
use cyrup_core::{
    CancelToken, Content, Cost, EventStream, Message, ModelRef, StopReason, Tool, ToolCallId,
    ToolError, ToolResult, ToolUpdateSink, Usage,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn model_ref() -> ModelRef {
    ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
}

fn obj_schema() -> Value {
    json!({ "type": "object" })
}

/// A distinctive [`Usage`] so an assertion cannot pass on a `Default`.
fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read: 3,
        cache_write: 4,
        cache_write_1h: Some(5),
        reasoning: Some(6),
        total_tokens: input + output,
        cost: Cost { input: 0.5, output: 1.5, cache_read: 0.0, cache_write: 0.0, total: 2.0 },
    }
}

/// Records the `Context.messages` of every provider request, then delegates to the faux provider.
struct PayloadRecordingStreamFn {
    inner: Arc<dyn StreamFn>,
    payloads: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl StreamFn for PayloadRecordingStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.payloads.lock().unwrap().push(ctx.messages.clone());
        self.inner.stream(model, ctx, opts)
    }
}

type PayloadLog = Arc<Mutex<Vec<Vec<Message>>>>;

fn payload_recording(
    responses: Vec<cyrup_core::AssistantMessage>,
) -> (Arc<dyn StreamFn>, PayloadLog) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;
    let inner: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> =
        Arc::new(PayloadRecordingStreamFn { inner, payloads: payloads.clone() });
    (sf, payloads)
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

/// The `turn_end.toolResults` of the FIRST turn that produced any.
fn first_turn_results(events: &[AgentEvent]) -> Vec<ToolResultMessage> {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { tool_results, .. } if !tool_results.is_empty() => {
                Some(tool_results.clone())
            }
            _ => None,
        })
        .expect("a turn_end carrying tool results")
}

/// The `message_end` payloads that carry a tool result.
fn message_end_results(events: &[AgentEvent]) -> Vec<ToolResultMessage> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageEnd { message: AgentMessage::ToolResult(t) } => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// The `tool_execution_end.result` payloads.
fn execution_end_results(events: &[AgentEvent]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd { result, .. } => Some(result.clone()),
            _ => None,
        })
        .collect()
}

/// Every `Message::ToolResult` in an LLM payload, as `(tool_name, usage, added_tool_names)`.
fn payload_tool_results(msgs: &[Message]) -> Vec<(String, Option<Usage>, Vec<String>)> {
    msgs.iter()
        .filter_map(|m| match m {
            Message::ToolResult { tool_name, usage, added_tool_names, .. } => {
                Some((tool_name.clone(), usage.clone(), added_tool_names.clone()))
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// A tool whose result carries a caller-supplied `usage` / `added_tool_names`.
struct ReportingTool {
    name: String,
    params: Value,
    usage: Option<Usage>,
    added: Vec<String>,
    calls: Arc<AtomicUsize>,
}

impl ReportingTool {
    fn new(name: &str, usage: Option<Usage>, added: &[&str]) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let t = Arc::new(Self {
            name: name.into(),
            params: obj_schema(),
            usage,
            added: added.iter().map(|s| (*s).to_string()).collect(),
            calls: calls.clone(),
        });
        (t, calls)
    }
}

#[async_trait::async_trait]
impl Tool for ReportingTool {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text(format!("ran:{}", self.name))],
            details: None,
            usage: self.usage.clone(),
            added_tool_names: self.added.clone(),
            terminate: false,
        })
    }
}

// ===========================================================================
// AGENT-005 — tool-reported usage
// ===========================================================================

/// A tool's `usage` reaches `tool_execution_end.result`, the transcript `ToolResultMessage`
/// (`message_end` + `turn_end.toolResults`) AND the LLM payload of the NEXT turn.
/// Pi: `createToolResultMessage` sets `usage: finalized.result.usage` (agent-loop.ts:782).
#[tokio::test]
async fn tool_reported_usage_surfaces_on_events_and_next_turn_payload() {
    let u = usage(11, 22);
    let (tool, _calls) = ReportingTool::new("meter", Some(u.clone()), &[]);
    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("meter", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();

    // 1. tool_execution_end.result carries the full AgentToolResult incl. usage.
    let ends = execution_end_results(&events);
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["usage"]["input"], json!(11));
    assert_eq!(ends[0]["usage"]["output"], json!(22));
    assert_eq!(ends[0]["usage"]["totalTokens"], json!(33));

    // 2. The transcript message carries it, on both message_end and turn_end.toolResults.
    let me = message_end_results(&events);
    assert_eq!(me.len(), 1);
    assert_eq!(me[0].usage.as_ref(), Some(&u), "message_end tool result carries usage");
    let te = first_turn_results(&events);
    assert_eq!(te[0].usage.as_ref(), Some(&u), "turn_end.toolResults carries usage");

    // 3. It survives `convert_to_llm` and reaches the provider on the following turn.
    let p = payloads.lock().unwrap().clone();
    assert_eq!(p.len(), 2, "two provider requests");
    let results = payload_tool_results(&p[1]);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1.as_ref(), Some(&u), "LLM payload tool result carries usage");
}

/// A tool that reports nothing leaves `usage` absent — and absent means NO KEY on the wire, not a
/// `null` (Pi omits an `undefined` `usage` via `JSON.stringify`).
#[tokio::test]
async fn absent_usage_emits_no_key() {
    let (tool, _calls) = ReportingTool::new("plain", None, &[]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("plain", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();
    let ends = execution_end_results(&events);
    assert!(ends[0].get("usage").is_none(), "no `usage` key at all: {}", ends[0]);
    assert!(ends[0].get("addedToolNames").is_none(), "no `addedToolNames` key: {}", ends[0]);

    let te = first_turn_results(&events);
    assert_eq!(te[0].usage, None);
    let json = serde_json::to_value(&te[0]).unwrap();
    assert!(json.get("usage").is_none(), "ToolResultMessage omits `usage`: {json}");
    assert!(json.get("addedToolNames").is_none(), "omits `addedToolNames`: {json}");
}

/// `after_tool_call` OBSERVES the tool's usage and REPLACES it wholesale (no deep merge).
/// Pi: `usage: afterResult.usage ?? result.usage` (agent-loop.ts:738).
struct UsagePatchHook {
    observed: Arc<Mutex<Vec<Option<Usage>>>>,
    replacement: Usage,
}

#[async_trait::async_trait]
impl Hooks for UsagePatchHook {
    async fn after_tool_call(
        &self,
        ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        self.observed.lock().unwrap().push(ctx.usage.cloned());
        Ok(Some(AfterOverride {
            usage: Some(self.replacement.clone()),
            ..AfterOverride::default()
        }))
    }
}

#[tokio::test]
async fn after_tool_call_observes_then_replaces_usage() {
    let from_tool = usage(11, 22);
    let from_hook = usage(700, 800);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let (tool, _calls) = ReportingTool::new("meter", Some(from_tool.clone()), &[]);
    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("meter", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![tool])
        .hooks(Arc::new(UsagePatchHook {
            observed: observed.clone(),
            replacement: from_hook.clone(),
        }))
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // READ side: the hook saw what the tool reported, not `None`.
    assert_eq!(observed.lock().unwrap().clone(), vec![Some(from_tool.clone())]);

    // WRITE side: the hook's value replaces it end to end.
    let events = rec.snapshot();
    let te = first_turn_results(&events);
    assert_eq!(te[0].usage.as_ref(), Some(&from_hook), "hook usage replaces the tool's");
    assert_ne!(te[0].usage.as_ref(), Some(&from_tool));
    let ends = execution_end_results(&events);
    assert_eq!(ends[0]["usage"]["input"], json!(700));
    let p = payloads.lock().unwrap().clone();
    assert_eq!(payload_tool_results(&p[1])[0].1.as_ref(), Some(&from_hook));
}

/// A hook that returns an override WITHOUT `usage` keeps the tool's value (`None` = keep).
struct ContentOnlyHook;

#[async_trait::async_trait]
impl Hooks for ContentOnlyHook {
    async fn after_tool_call(
        &self,
        _ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        Ok(Some(AfterOverride {
            content: Some(vec![Content::text("patched")]),
            ..AfterOverride::default()
        }))
    }
}

#[tokio::test]
async fn override_without_usage_keeps_the_tools_usage_and_anchor() {
    let u = usage(11, 22);
    let (tool, _calls) = ReportingTool::new("meter", Some(u.clone()), &["late"]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("meter", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![tool])
        .hooks(Arc::new(ContentOnlyHook))
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    match &te[0].content[0] {
        Content::Text { text, .. } => assert_eq!(text, "patched"),
        other => panic!("expected patched text, got {other:?}"),
    }
    assert_eq!(te[0].usage.as_ref(), Some(&u), "an omitted override field keeps the original");
    // Pi's `AfterToolCallResult` has no `addedToolNames`; the tool's value rides the
    // `{...result}` spread untouched (agent-loop.ts:736-742).
    assert_eq!(te[0].added_tool_names, vec!["late".to_string()], "hook cannot clear the anchor");
}

/// A THROWING `after_tool_call` discards the whole result — Pi replaces it with
/// `createErrorToolResult(...)` (agent-loop.ts:744-747), which has neither usage nor added tools.
struct ThrowingHook;

#[async_trait::async_trait]
impl Hooks for ThrowingHook {
    async fn after_tool_call(
        &self,
        _ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        Err(HookError::new("boom"))
    }
}

#[tokio::test]
async fn throwing_after_tool_call_clears_usage_and_anchor() {
    let (tool, _calls) = ReportingTool::new("meter", Some(usage(11, 22)), &["late"]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("meter", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent =
        Agent::builder(model_ref(), sf).tools(vec![tool]).hooks(Arc::new(ThrowingHook)).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert!(te[0].is_error);
    assert_eq!(te[0].usage, None, "the error result carries no usage");
    assert!(te[0].added_tool_names.is_empty(), "the error result anchors nothing");
}

/// A result that never ran (unknown tool ⇒ `immediate_error`, Pi `createErrorToolResult`) carries
/// neither field. This also covers `fail_truncated_tool_calls`, which builds via the same helper.
#[tokio::test]
async fn immediate_error_result_carries_neither_field() {
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("nope", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();
    let te = first_turn_results(&events);
    assert!(te[0].is_error);
    assert_eq!(te[0].usage, None);
    assert!(te[0].added_tool_names.is_empty());
    let ends = execution_end_results(&events);
    assert!(ends[0].get("usage").is_none());
    assert!(ends[0].get("addedToolNames").is_none());
}

// ===========================================================================
// AGENT-004 — the `added_tool_names` transcript anchor
// ===========================================================================

/// The load point is anchored to the EXACT tool result that introduced the tools, and to no
/// earlier message — the property a provider adapter with native deferred tool loading reads to
/// decide prefix-vs-transcript placement (Pi `splitDeferredTools`, ai/src/utils/deferred-tools.ts).
///
/// Turn 1: the model calls `loader`, whose result announces `["late"]`.
/// Turn 2: the model calls `late` — it executes normally, i.e. it IS callable from that point on.
/// Turn 3: the model stops.
#[tokio::test]
async fn added_tool_names_anchor_lands_on_the_introducing_result_and_nowhere_earlier() {
    let (loader, loader_calls) = ReportingTool::new("loader", None, &["late"]);
    let (late, late_calls) = ReportingTool::new("late", None, &[]);
    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("loader", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_tool_call("late", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![loader, late]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(late_calls.load(Ordering::SeqCst), 1, "`late` was actually invoked");

    let p = payloads.lock().unwrap().clone();
    assert_eq!(p.len(), 3, "three provider requests");

    // --- Turn 1's payload predates the anchor entirely: no tool result at all.
    assert!(payload_tool_results(&p[0]).is_empty(), "nothing anchored before the tool ran");

    // --- Turn 2's payload: exactly one anchor, on `loader`, at the transcript index of its result.
    let anchored: Vec<usize> = p[1]
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            matches!(m, Message::ToolResult { added_tool_names, .. } if !added_tool_names.is_empty())
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(anchored.len(), 1, "exactly one anchoring message");
    let anchor_idx = anchored[0];
    match &p[1][anchor_idx] {
        Message::ToolResult { tool_name, added_tool_names, .. } => {
            assert_eq!(tool_name, "loader");
            assert_eq!(added_tool_names, &vec!["late".to_string()]);
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
    // Every message BEFORE the anchor is anchor-free, and serializes without the key at all.
    for (i, m) in p[1].iter().enumerate().take(anchor_idx) {
        let v = serde_json::to_value(m).unwrap();
        assert!(v.get("addedToolNames").is_none(), "message {i} must not carry an anchor: {v}");
    }

    // --- Turn 3's payload: the anchor stays put; `late`'s own result introduces nothing new.
    let t3 = payload_tool_results(&p[2]);
    assert_eq!(
        t3,
        vec![
            ("loader".to_string(), None, vec!["late".to_string()]),
            ("late".to_string(), None, Vec::new()),
        ],
        "the anchor stays on the introducing result across later turns"
    );
    // And it is still at the SAME transcript index it was assigned on turn 2.
    match &p[2][anchor_idx] {
        Message::ToolResult { tool_name, added_tool_names, .. } => {
            assert_eq!(tool_name, "loader");
            assert_eq!(added_tool_names, &vec!["late".to_string()]);
        }
        other => panic!("anchor moved; index {anchor_idx} is now {other:?}"),
    }
}

/// Two tools announcing in the same parallel batch keep their own anchors, in source order —
/// `execute_parallel`'s two-phase prepare/execute split must not lose or merge them.
#[tokio::test]
async fn parallel_batch_preserves_per_result_anchors_in_source_order() {
    let (a, _) = ReportingTool::new("a", Some(usage(1, 2)), &["x"]);
    let (b, _) = ReportingTool::new("b", Some(usage(3, 4)), &["y", "z"]);
    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("a", json!({})), faux_tool_call("b", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![a, b]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert_eq!(te.len(), 2);
    assert_eq!(te[0].tool_name, "a");
    assert_eq!(te[0].added_tool_names, vec!["x".to_string()]);
    assert_eq!(te[0].usage, Some(usage(1, 2)));
    assert_eq!(te[1].tool_name, "b");
    assert_eq!(te[1].added_tool_names, vec!["y".to_string(), "z".to_string()]);
    assert_eq!(te[1].usage, Some(usage(3, 4)));

    let p = payloads.lock().unwrap().clone();
    assert_eq!(
        payload_tool_results(&p[1]),
        vec![
            ("a".to_string(), Some(usage(1, 2)), vec!["x".to_string()]),
            ("b".to_string(), Some(usage(3, 4)), vec!["y".to_string(), "z".to_string()]),
        ]
    );
}

/// The same, for the sequential path (`execute_sequential`) — one `Sequential` tool forces the
/// whole batch sequential, so this exercises a different threading site.
#[tokio::test]
async fn sequential_batch_preserves_per_result_anchors() {
    struct SeqTool(Arc<ReportingTool>);
    #[async_trait::async_trait]
    impl Tool for SeqTool {
        fn name(&self) -> &str {
            self.0.name()
        }
        fn parameters(&self) -> &Value {
            self.0.parameters()
        }
        fn execution_mode(&self) -> cyrup_core::ExecMode {
            cyrup_core::ExecMode::Sequential
        }
        async fn execute(
            &self,
            call_id: ToolCallId,
            params: Value,
            cancel: CancelToken,
            on_update: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            self.0.execute(call_id, params, cancel, on_update).await
        }
    }

    let (a, _) = ReportingTool::new("a", Some(usage(1, 2)), &["x"]);
    let (b, _) = ReportingTool::new("b", None, &["y"]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("a", json!({})), faux_tool_call("b", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![Arc::new(SeqTool(a)) as Arc<dyn Tool>, b])
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert_eq!(te.len(), 2);
    assert_eq!(te[0].added_tool_names, vec!["x".to_string()]);
    assert_eq!(te[0].usage, Some(usage(1, 2)));
    assert_eq!(te[1].added_tool_names, vec!["y".to_string()]);
    assert_eq!(te[1].usage, None);
}

/// A failing tool (`Err(ToolError)`) anchors nothing — it never produced a result to anchor to.
#[tokio::test]
async fn failing_tool_anchors_nothing() {
    struct Boom(Value);
    #[async_trait::async_trait]
    impl Tool for Boom {
        fn name(&self) -> &str {
            "boom"
        }
        fn parameters(&self) -> &Value {
            &self.0
        }
        async fn execute(
            &self,
            _c: ToolCallId,
            _p: Value,
            _x: CancelToken,
            _u: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            Err(ToolError::new("nope"))
        }
    }
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("boom", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![Arc::new(Boom(obj_schema()))]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert!(te[0].is_error);
    assert!(te[0].added_tool_names.is_empty());
    assert_eq!(te[0].usage, None);
}

/// PRIVILEGE ESCALATION — a tool announced by `added_tool_names` is gated exactly like any other.
///
/// `RunCtx::prepare` calls `hooks.before_tool_call` unconditionally for EVERY call
/// (`agent.rs`, `self.hooks.before_tool_call(ctx, …)`), keyed on the tool NAME and arguments at
/// call time — never on how or when the tool entered the tool list. In the real wiring that chain
/// is `PolicyHooks::before_tool_call` → `ExtHooks` → `HostEvent::ToolCall`, which is what
/// `PermissionSystemExtension` subscribes to. Here a blocking hook stands in for the gate: the
/// announced tool is blocked and never executes, while an ordinary call in the same run proceeds.
struct GateLateToolHook;

#[async_trait::async_trait]
impl Hooks for GateLateToolHook {
    async fn before_tool_call(
        &self,
        ctx: crate::BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<crate::BeforeOutcome, HookError> {
        if ctx.tool_name == "late" {
            Ok(crate::BeforeOutcome::Block { reason: Some("denied by policy".into()), terminate: false })
        } else {
            Ok(crate::BeforeOutcome::Proceed)
        }
    }
}

#[tokio::test]
async fn an_announced_tool_is_still_subject_to_the_permission_gate() {
    let (loader, loader_calls) = ReportingTool::new("loader", None, &["late"]);
    let (late, late_calls) = ReportingTool::new("late", None, &[]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("loader", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_tool_call("late", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![loader, late])
        .hooks(Arc::new(GateLateToolHook))
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // The announcing tool ran; the announced one was gated BEFORE execution.
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(late_calls.load(Ordering::SeqCst), 0, "the gate ran before `late` could execute");

    let results = message_end_results(&rec.snapshot());
    let blocked = results.iter().find(|r| r.tool_name == "late").expect("a result for `late`");
    assert!(blocked.is_error, "the gated call produces an error result");
    match &blocked.content[0] {
        Content::Text { text, .. } => assert_eq!(text, "denied by policy"),
        other => panic!("expected the block reason, got {other:?}"),
    }
    // A blocked call cannot smuggle an anchor or usage through the gate.
    assert!(blocked.added_tool_names.is_empty());
    assert_eq!(blocked.usage, None);
}

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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    AfterOutcome, AfterOverride, AfterToolCall, Agent, AgentEvent, AgentMessage, HookError, Hooks,
    ToolResultMessage,
};
use cyrup_core::{
    CancelToken, Content, Cost, Message, StopReason, TerminateHint, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink, Usage,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use serde_json::{Value, json};

use super::support::*;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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
        cost: Cost {
            input: 0.5,
            output: 1.5,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 2.0,
        },
    }
}

/// The `message_end` payloads that carry a tool result.
fn message_end_results(events: &[AgentEvent]) -> Vec<ToolResultMessage> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageEnd {
                message: AgentMessage::ToolResult(t),
            } => Some(t.clone()),
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
            Message::ToolResult {
                tool_name,
                usage,
                added_tool_names,
                ..
            } => Some((tool_name.clone(), usage.clone(), added_tool_names.clone())),
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
    /// AGENT-045 — the machine-readable half, set only by the structured-output cases.
    structured: Option<Value>,
    /// AGENT-045 — what [`Tool::output_schema`] answers for this tool.
    schema: Option<Value>,
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
            structured: None,
            schema: None,
            calls: calls.clone(),
        });
        (t, calls)
    }

    /// AGENT-045 — a tool that declares an `output_schema` and returns a matching
    /// `structured_content`, which is what pi's `bash` does (`core/tools/bash.ts:259`, `:391`).
    fn structured(name: &str, schema: Value, structured: Value) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            params: obj_schema(),
            usage: None,
            added: Vec::new(),
            structured: Some(structured),
            schema: Some(schema),
            calls: Arc::new(AtomicUsize::new(0)),
        })
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
    fn output_schema(&self) -> Option<&Value> {
        self.schema.as_ref()
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
            structured_content: self.structured.clone(),
            // AGENT-046 — this fixture exercises the SUCCESS shape; the tool-reported-failure
            // shape has its own file (`agent046_tool_reported_failure.rs`).
            is_error: false,
            terminate: TerminateHint::Unspecified,
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
        faux_assistant_message(
            vec![faux_tool_call("meter", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
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
    assert_eq!(
        me[0].usage.as_ref(),
        Some(&u),
        "message_end tool result carries usage"
    );
    let te = first_turn_results(&events);
    assert_eq!(
        te[0].usage.as_ref(),
        Some(&u),
        "turn_end.toolResults carries usage"
    );

    // 3. It survives `convert_to_llm` and reaches the provider on the following turn.
    let p = payloads.lock().unwrap().clone();
    assert_eq!(p.len(), 2, "two provider requests");
    let results = payload_tool_results(&p[1]);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].1.as_ref(),
        Some(&u),
        "LLM payload tool result carries usage"
    );
}

/// A tool that reports nothing leaves `usage` absent — and absent means NO KEY on the wire, not a
/// `null` (Pi omits an `undefined` `usage` via `JSON.stringify`).
#[tokio::test]
async fn absent_usage_emits_no_key() {
    let (tool, _calls) = ReportingTool::new("plain", None, &[]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("plain", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();
    let ends = execution_end_results(&events);
    assert!(
        ends[0].get("usage").is_none(),
        "no `usage` key at all: {}",
        ends[0]
    );
    assert!(
        ends[0].get("addedToolNames").is_none(),
        "no `addedToolNames` key: {}",
        ends[0]
    );

    let te = first_turn_results(&events);
    assert_eq!(te[0].usage, None);
    let json = serde_json::to_value(&te[0]).unwrap();
    assert!(
        json.get("usage").is_none(),
        "ToolResultMessage omits `usage`: {json}"
    );
    assert!(
        json.get("addedToolNames").is_none(),
        "omits `addedToolNames`: {json}"
    );
}

/// `after_tool_call` OBSERVES the tool's usage and REPLACES it wholesale (no deep merge).
/// Pi: `usage: afterResult.usage ?? result.usage` (agent-loop.ts:738).
struct UsagePatchHook {
    observed: Arc<Mutex<Vec<Option<Usage>>>>,
    replacement: Usage,
}

#[async_trait::async_trait]
impl Hooks for UsagePatchHook {
    async fn after_tool_call(&self, ctx: AfterToolCall<'_>, _cancel: CancelToken) -> AfterOutcome {
        self.observed.lock().unwrap().push(ctx.usage.cloned());
        AfterOutcome::Override(Box::new(AfterOverride {
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
        faux_assistant_message(
            vec![faux_tool_call("meter", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
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
    assert_eq!(
        observed.lock().unwrap().clone(),
        vec![Some(from_tool.clone())]
    );

    // WRITE side: the hook's value replaces it end to end.
    let events = rec.snapshot();
    let te = first_turn_results(&events);
    assert_eq!(
        te[0].usage.as_ref(),
        Some(&from_hook),
        "hook usage replaces the tool's"
    );
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
    async fn after_tool_call(&self, _ctx: AfterToolCall<'_>, _cancel: CancelToken) -> AfterOutcome {
        AfterOutcome::Override(Box::new(AfterOverride {
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
        faux_assistant_message(
            vec![faux_tool_call("meter", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
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
    assert_eq!(
        te[0].usage.as_ref(),
        Some(&u),
        "an omitted override field keeps the original"
    );
    // Pi's `AfterToolCallResult` has no `addedToolNames`; the tool's value rides the
    // `{...result}` spread untouched (agent-loop.ts:736-742).
    assert_eq!(
        te[0].added_tool_names,
        vec!["late".to_string()],
        "hook cannot clear the anchor"
    );
}

/// A THROWING `after_tool_call` discards the whole result — Pi replaces it with
/// `createErrorToolResult(...)` (agent-loop.ts:744-747), which has neither usage nor added tools.
struct ThrowingHook;

#[async_trait::async_trait]
impl Hooks for ThrowingHook {
    async fn after_tool_call(&self, _ctx: AfterToolCall<'_>, _cancel: CancelToken) -> AfterOutcome {
        AfterOutcome::Failed(HookError::new("boom"))
    }
}

#[tokio::test]
async fn throwing_after_tool_call_clears_usage_and_anchor() {
    let (tool, _calls) = ReportingTool::new("meter", Some(usage(11, 22)), &["late"]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("meter", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![tool])
        .hooks(Arc::new(ThrowingHook))
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert!(te[0].is_error);
    assert_eq!(te[0].usage, None, "the error result carries no usage");
    assert!(
        te[0].added_tool_names.is_empty(),
        "the error result anchors nothing"
    );
}

/// A result that never ran (unknown tool ⇒ `immediate_error`, Pi `createErrorToolResult`) carries
/// neither field. This also covers `fail_truncated_tool_calls`, which builds via the same helper.
#[tokio::test]
async fn immediate_error_result_carries_neither_field() {
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("nope", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
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
        faux_assistant_message(
            vec![faux_tool_call("loader", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_tool_call("late", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![loader, late])
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        late_calls.load(Ordering::SeqCst),
        1,
        "`late` was actually invoked"
    );

    let p = payloads.lock().unwrap().clone();
    assert_eq!(p.len(), 3, "three provider requests");

    // --- Turn 1's payload predates the anchor entirely: no tool result at all.
    assert!(
        payload_tool_results(&p[0]).is_empty(),
        "nothing anchored before the tool ran"
    );

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
        Message::ToolResult {
            tool_name,
            added_tool_names,
            ..
        } => {
            assert_eq!(tool_name, "loader");
            assert_eq!(added_tool_names, &vec!["late".to_string()]);
        }
        other => panic!("expected a tool result, got {other:?}"),
    }
    // Every message BEFORE the anchor is anchor-free, and serializes without the key at all.
    for (i, m) in p[1].iter().enumerate().take(anchor_idx) {
        let v = serde_json::to_value(m).unwrap();
        assert!(
            v.get("addedToolNames").is_none(),
            "message {i} must not carry an anchor: {v}"
        );
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
        Message::ToolResult {
            tool_name,
            added_tool_names,
            ..
        } => {
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
            vec![
                faux_tool_call("a", json!({})),
                faux_tool_call("b", json!({})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
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
    assert_eq!(
        te[1].added_tool_names,
        vec!["y".to_string(), "z".to_string()]
    );
    assert_eq!(te[1].usage, Some(usage(3, 4)));

    let p = payloads.lock().unwrap().clone();
    assert_eq!(
        payload_tool_results(&p[1]),
        vec![
            ("a".to_string(), Some(usage(1, 2)), vec!["x".to_string()]),
            (
                "b".to_string(),
                Some(usage(3, 4)),
                vec!["y".to_string(), "z".to_string()]
            ),
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
            vec![
                faux_tool_call("a", json!({})),
                faux_tool_call("b", json!({})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
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
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![Arc::new(Boom(obj_schema()))])
        .build();
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
    ) -> crate::BeforeOutcome {
        if ctx.tool_name == "late" {
            crate::BeforeOutcome::Block {
                reason: Some("denied by policy".into()),
                terminate: TerminateHint::Unspecified,
            }
        } else {
            crate::BeforeOutcome::Proceed
        }
    }
}

#[tokio::test]
async fn an_announced_tool_is_still_subject_to_the_permission_gate() {
    let (loader, loader_calls) = ReportingTool::new("loader", None, &["late"]);
    let (late, late_calls) = ReportingTool::new("late", None, &[]);
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("loader", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_tool_call("late", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![loader, late])
        .hooks(Arc::new(GateLateToolHook))
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // The announcing tool ran; the announced one was gated BEFORE execution.
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        late_calls.load(Ordering::SeqCst),
        0,
        "the gate ran before `late` could execute"
    );

    let results = message_end_results(&rec.snapshot());
    let blocked = results
        .iter()
        .find(|r| r.tool_name == "late")
        .expect("a result for `late`");
    assert!(blocked.is_error, "the gated call produces an error result");
    match &blocked.content[0] {
        Content::Text { text, .. } => assert_eq!(text, "denied by policy"),
        other => panic!("expected the block reason, got {other:?}"),
    }
    // A blocked call cannot smuggle an anchor or usage through the gate.
    assert!(blocked.added_tool_names.is_empty());
    assert_eq!(blocked.usage, None);
}

// ===========================================================================
// AGENT-045 — `outputSchema` / `structuredContent`
// ===========================================================================

/// AGENT-045 — a tool's `structured_content` reaches the `tool_execution_end.result` payload and
/// is **never sent to the model**.
///
/// pi `AgentToolResult.structuredContent` (`packages/agent/src/types.ts:429-433` @v1.0.0):
/// *"Machine-readable result matching the tool's `outputSchema`, for programmatic callers. Not
/// sent to the model; `content` remains the model-facing result."* `emitToolExecutionEnd` emits
/// `result: finalized.result` verbatim so the event carries it, while the transcript
/// `ToolResultMessage` (`packages/ai/src/types.ts`) has no such field at v1.0.0 — so neither the
/// persisted message nor the next turn's provider payload can carry it.
///
/// This is the assertion that makes the field safe to add: a tool can hand a programmatic caller
/// a typed object without widening what the model sees, and without growing the transcript.
///
/// RED before the fix: `ToolResult` had no `structured_content` field, so `ReportingTool` did not
/// compile; with the field but without threading it to `result_value_of`, the
/// `tool_execution_end` assertion fails.
#[tokio::test]
async fn agent045_structured_content_reaches_the_event_but_not_the_model() {
    let schema = json!({ "type": "object", "properties": { "exitCode": { "type": "number" } } });
    let structured = json!({ "exitCode": 0, "output": "AGENT-045-STRUCTURED" });
    let tool = ReportingTool::structured("probe", schema, structured.clone());
    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("probe", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();

    // 1. The event payload carries the structured half, under pi's camelCase key.
    let ends = execution_end_results(&events);
    assert_eq!(ends.len(), 1);
    assert_eq!(
        ends[0].get("structuredContent"),
        Some(&structured),
        "`tool_execution_end.result` must carry it (pi emits `finalized.result` verbatim): {}",
        ends[0]
    );
    // …and `content` is still the model-facing result, untouched.
    assert_eq!(
        ends[0]["content"][0]["text"],
        json!("ran:probe"),
        "`content` remains the model-facing result"
    );

    // 2. The transcript message does NOT gain the key — pi's `ToolResultMessage` has no such
    //    field, so a session file never records one.
    let me = message_end_results(&events);
    assert_eq!(me.len(), 1);
    let as_json = serde_json::to_value(&me[0]).expect("the message serializes");
    assert!(
        as_json.get("structuredContent").is_none(),
        "the persisted tool-result message must not carry the structured half: {as_json}"
    );

    // 3. The decisive one: it never reaches the provider. Scan the WHOLE next-turn request, not
    //    just its tool-result blocks, so the value cannot slip in through any other field.
    let p = payloads.lock().unwrap().clone();
    assert_eq!(p.len(), 2, "two provider requests");
    let wire = serde_json::to_string(&p[1]).expect("the payload serializes");
    assert!(
        wire.contains("ran:probe"),
        "sanity: the model DOES see the text content"
    );
    assert!(
        !wire.contains("AGENT-045-STRUCTURED"),
        "the structured half must never be sent to the model (types.ts:429-432): {wire}"
    );
    assert!(
        !wire.contains("structuredContent"),
        "not even the key reaches the provider payload: {wire}"
    );
}

/// A tool that declares no `output_schema` and returns no structured half puts NO key on the wire
/// — absent, not `null` (pi's `delete result.structuredContent`, `agent-loop.ts:888`).
#[tokio::test]
async fn agent045_a_tool_without_an_output_schema_emits_no_structured_key() {
    let (tool, _calls) = ReportingTool::new("plain", None, &[]);
    assert!(
        cyrup_core::Tool::output_schema(tool.as_ref()).is_none(),
        "a text-only tool declares no output schema"
    );
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(
            vec![faux_tool_call("plain", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let ends = execution_end_results(&rec.snapshot());
    assert!(
        ends[0].get("structuredContent").is_none(),
        "no `structuredContent` key at all: {}",
        ends[0]
    );
}

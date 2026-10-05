//! CODE-006 — `NestedToolCallRunner`: the calls a tool makes while it runs, their ids, events,
//! record, usage and exclusive queue.
//!
//! Ports `packages/coding-agent/test/nested-tool-calls.test.ts` @v1.0.1 case for case (the
//! `NestedToolCallRunner` describe block; the `NestedCallRecorder` block is
//! `cyrup_core::message::nested`'s own tests), plus the queue-ordering and nested-in-nested cases
//! the deadlock rule (`holdsQueue`) exists for.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cyrup_core::{
    CancelToken, Content, ExecMode, NestedCallStatus, Tool, ToolCall, ToolCallId, ToolError,
    ToolResult, ToolUpdate, ToolUpdateSink, Usage,
};
use serde_json::{Value, json};

use super::support::obj_schema;
use crate::agent::{
    NestedToolCallHost, NestedToolCallOptions, NestedToolCallRunner, NestedToolExecutionEvent,
};
use crate::{AgentEvent, ToolCallOutcome};

fn usage(input: u64, cost: f64) -> Usage {
    Usage {
        input,
        total_tokens: input,
        cost: cyrup_core::Cost {
            input: cost,
            total: cost,
            ..cyrup_core::Cost::default()
        },
        ..Usage::default()
    }
}

/// The host of pi's `createRunner`: resolves against `tools`, runs the tool body directly.
struct TestHost {
    tools: Mutex<Vec<Arc<dyn Tool>>>,
    sequential: bool,
    events: Mutex<Vec<NestedToolExecutionEvent>>,
}

impl TestHost {
    fn new(tools: Vec<Arc<dyn Tool>>, sequential: bool) -> Arc<Self> {
        Arc::new(Self {
            tools: Mutex::new(tools),
            sequential,
            events: Mutex::new(Vec::new()),
        })
    }

    fn push(&self, tool: Arc<dyn Tool>) {
        self.tools.lock().unwrap().push(tool);
    }

    /// `[type, toolCallId, parentToolCallId]` of every event seen, as the pi test compares them.
    fn event_rows(&self) -> Vec<(String, String, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|event| {
                let wire = serde_json::to_value(event).unwrap();
                let field = |key: &str| wire[key].as_str().unwrap().to_string();
                (
                    field("type"),
                    field("toolCallId"),
                    field("parentToolCallId"),
                )
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl NestedToolCallHost for TestHost {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.lock().unwrap().clone()
    }

    fn is_sequential(&self) -> bool {
        self.sequential
    }

    async fn run_tool_call(
        &self,
        call: ToolCall,
        _parent: &ToolCallId,
        cancel: Option<CancelToken>,
        on_update: ToolUpdateSink,
    ) -> ToolCallOutcome {
        let tool = self
            .tools()
            .into_iter()
            .find(|candidate| candidate.name() == call.name);
        let Some(tool) = tool else {
            return ToolCallOutcome {
                result: ToolResult {
                    content: vec![Content::text(format!("Tool {} not found", call.name))],
                    details: Some(json!({})),
                    ..ToolResult::default()
                },
                is_error: true,
                tool_call: call,
            };
        };
        let result = tool
            .execute(
                call.id.clone(),
                Value::Object((*call.arguments).clone()),
                cancel.unwrap_or_default(),
                on_update,
            )
            .await;
        match result {
            Ok(result) => ToolCallOutcome {
                is_error: result.is_error,
                result,
                tool_call: call,
            },
            Err(e) => ToolCallOutcome {
                result: ToolResult {
                    content: vec![Content::text(e.message)],
                    ..ToolResult::default()
                },
                is_error: true,
                tool_call: call,
            },
        }
    }

    async fn emit(&self, event: NestedToolExecutionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// A tool built from a closure, so each case spells its body inline.
struct FnTool {
    name: &'static str,
    mode: ExecMode,
    params: Value,
    body: Box<dyn Fn(ToolCallId, ToolUpdateSink) -> BodyFuture + Send + Sync>,
}

type BodyFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult, ToolError>> + Send>>;

impl FnTool {
    fn new(
        name: &'static str,
        body: impl Fn(ToolCallId, ToolUpdateSink) -> BodyFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            mode: ExecMode::Parallel,
            params: obj_schema(),
            body: Box::new(body),
        }
    }

    fn sequential(mut self) -> Self {
        self.mode = ExecMode::Sequential;
        self
    }
}

#[async_trait::async_trait]
impl Tool for FnTool {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn execution_mode(&self) -> ExecMode {
        self.mode
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        (self.body)(call_id, on_update).await
    }
}

/// One nested call with empty arguments and default options.
async fn run_nested(
    runner: &NestedToolCallRunner,
    host: &Arc<TestHost>,
    caller: &ToolCallId,
    name: &str,
) -> ToolCallOutcome {
    runner
        .execute(
            &**host,
            caller,
            name,
            json!({}),
            NestedToolCallOptions::default(),
        )
        .await
}

fn arc(tool: FnTool) -> Arc<dyn Tool> {
    Arc::new(tool)
}

fn id(s: &str) -> ToolCallId {
    ToolCallId::from(s)
}

/// nested-tool-calls.test.ts "assigns ids below the caller, emits events with the parent id, and
/// records the calls".
#[tokio::test]
async fn assigns_ids_below_the_caller_emits_events_with_the_parent_id_and_records_the_calls() {
    let echo = arc(FnTool::new("echo", |_id, mut on_update| {
        Box::pin(async move {
            on_update(ToolUpdate {
                content: vec![Content::text("partial")],
                ..ToolUpdate::default()
            });
            Ok(ToolResult {
                content: vec![Content::text("ok")],
                ..ToolResult::default()
            })
        })
    }));
    let host = TestHost::new(vec![echo], false);
    let runner = NestedToolCallRunner::new();
    let updates = Arc::new(Mutex::new(Vec::<ToolUpdate>::new()));
    let sink = updates.clone();

    let first = runner
        .execute(
            &*host,
            &id("call"),
            "echo",
            json!({ "a": 1 }),
            NestedToolCallOptions {
                cancel: None,
                on_update: Some(Box::new(move |u| sink.lock().unwrap().push(u))),
            },
        )
        .await;
    let missing = runner
        .execute(
            &*host,
            &id("call"),
            "missing",
            json!({}),
            NestedToolCallOptions::default(),
        )
        .await;

    assert_eq!(first.tool_call.id, id("call/1"));
    assert_eq!(missing.tool_call.id, id("call/2"));
    assert!(missing.is_error);
    assert_eq!(updates.lock().unwrap().len(), 1);
    let rows = host.event_rows();
    let want = |k: &str, c: &str| (k.to_string(), c.to_string(), "call".to_string());
    assert_eq!(
        rows,
        vec![
            want("tool_execution_start", "call/1"),
            want("tool_execution_update", "call/1"),
            want("tool_execution_end", "call/1"),
            want("tool_execution_start", "call/2"),
            want("tool_execution_end", "call/2"),
        ]
    );

    let summary = runner.take_record(&id("call")).unwrap();
    let calls = summary.calls.unwrap();
    assert!(calls.complete);
    assert_eq!(calls.calls.len(), 2);
    assert_eq!(calls.calls[0].id, "call/1");
    assert_eq!(calls.calls[0].name, "echo");
    assert_eq!(
        calls.calls[0].arguments,
        json!({ "a": 1 }).as_object().cloned()
    );
    assert_eq!(calls.calls[0].status, NestedCallStatus::Ok);
    assert!(calls.calls[0].duration_ms.is_some());
    assert_eq!(calls.calls[1].id, "call/2");
    assert_eq!(calls.calls[1].arguments, json!({}).as_object().cloned());
    assert_eq!(calls.calls[1].status, NestedCallStatus::Error);
    assert_eq!(
        calls.calls[1].error.as_deref(),
        Some("Tool missing not found")
    );
    // The record is taken once.
    assert!(runner.take_record(&id("call")).is_none());
    assert!(runner.take_record(&id("other")).is_none());
}

/// nested-tool-calls.test.ts "records calls of nested tools on the model-issued call".
#[tokio::test]
async fn records_calls_of_nested_tools_on_the_model_issued_call() {
    let leaf = arc(FnTool::new("leaf", |_, _| {
        Box::pin(async { Ok(ToolResult::default()) })
    }));
    let host = TestHost::new(vec![leaf], false);
    let runner = Arc::new(NestedToolCallRunner::new());
    let (h, r) = (host.clone(), runner.clone());
    host.push(arc(FnTool::new("middle", move |call_id, _| {
        let (h, r) = (h.clone(), r.clone());
        Box::pin(async move {
            r.execute(
                &*h,
                &call_id,
                "leaf",
                json!({}),
                NestedToolCallOptions::default(),
            )
            .await;
            Ok(ToolResult::default())
        })
    })));

    runner
        .execute(
            &*host,
            &id("call"),
            "middle",
            json!({}),
            NestedToolCallOptions::default(),
        )
        .await;

    let ids: Vec<_> = runner
        .take_record(&id("call"))
        .unwrap()
        .calls
        .unwrap()
        .calls
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, vec!["call/1".to_string(), "call/1/1".to_string()]);
}

/// nested-tool-calls.test.ts "sums the usage of nested results at every depth".
#[tokio::test]
async fn sums_the_usage_of_nested_results_at_every_depth() {
    let leaf = arc(FnTool::new("leaf", |_, _| {
        Box::pin(async {
            Ok(ToolResult {
                usage: Some(usage(10, 0.01)),
                ..ToolResult::default()
            })
        })
    }));
    let plain = arc(FnTool::new("plain", |_, _| {
        Box::pin(async { Ok(ToolResult::default()) })
    }));
    let host = TestHost::new(vec![leaf, plain], false);
    let runner = Arc::new(NestedToolCallRunner::new());
    let (h, r) = (host.clone(), runner.clone());
    host.push(arc(FnTool::new("middle", move |call_id, _| {
        let (h, r) = (h.clone(), r.clone());
        Box::pin(async move {
            r.execute(
                &*h,
                &call_id,
                "leaf",
                json!({}),
                NestedToolCallOptions::default(),
            )
            .await;
            // Its own usage only: the leaf's usage is counted once, by the recorder.
            Ok(ToolResult {
                usage: Some(usage(5, 0.005)),
                ..ToolResult::default()
            })
        })
    })));

    let (call, free) = (id("call"), id("free"));
    run_nested(&runner, &host, &call, "middle").await;
    run_nested(&runner, &host, &call, "leaf").await;
    run_nested(&runner, &host, &call, "plain").await;
    run_nested(&runner, &host, &free, "plain").await;

    let summary = runner.take_record(&id("call")).unwrap();
    let total = summary.usage.unwrap();
    assert_eq!(total.input, 25);
    assert!((total.cost.total - 0.025).abs() < 1e-10);
    let free = runner.take_record(&id("free")).unwrap();
    assert!(free.calls.unwrap().complete);
    assert_eq!(free.usage, None);
}

/// `clear` forgets every open scope: a model-issued call whose result never arrived leaves nothing
/// behind to leak into the next run (pi `_nestedToolCalls.clear()` on `agent_end`).
#[tokio::test]
async fn clear_forgets_every_open_scope() {
    let plain = arc(FnTool::new("plain", |_, _| {
        Box::pin(async { Ok(ToolResult::default()) })
    }));
    let host = TestHost::new(vec![plain], false);
    let runner = NestedToolCallRunner::new();
    let (a, b) = (id("a"), id("b"));
    run_nested(&runner, &host, &a, "plain").await;
    run_nested(&runner, &host, &b, "plain").await;
    runner.clear();
    assert!(runner.take_record(&a).is_none());
    assert!(runner.take_record(&b).is_none());
    // A new call under the same id starts a fresh record and numbering.
    let again = run_nested(&runner, &host, &a, "plain").await;
    assert_eq!(again.tool_call.id, id("a/1"));
}

/// A tool that tracks how many copies of its kind run at once.
fn counting_tool(
    name: &'static str,
    sequential: bool,
    active: Arc<AtomicUsize>,
    max: Arc<AtomicUsize>,
) -> Arc<dyn Tool> {
    let tool = FnTool::new(name, move |_, _| {
        let (active, max) = (active.clone(), max.clone());
        Box::pin(async move {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            max.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolResult::default())
        })
    });
    arc(if sequential { tool.sequential() } else { tool })
}

/// nested-tool-calls.test.ts "serializes concurrent calls to sequential tools".
#[tokio::test]
async fn serializes_concurrent_calls_to_sequential_tools() {
    let (seq_active, seq_max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (par_active, par_max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let host = TestHost::new(
        vec![
            counting_tool("sequential", true, seq_active, seq_max.clone()),
            counting_tool("parallel", false, par_active, par_max.clone()),
        ],
        false,
    );
    let runner = NestedToolCallRunner::new();
    let call = id("call");
    let run = |name: &'static str| {
        runner.execute(
            &*host,
            &call,
            name,
            json!({}),
            NestedToolCallOptions::default(),
        )
    };

    futures_join3(run("sequential"), run("sequential"), run("sequential")).await;
    futures_join3(run("parallel"), run("parallel"), run("parallel")).await;

    assert_eq!(seq_max.load(Ordering::SeqCst), 1);
    assert_eq!(par_max.load(Ordering::SeqCst), 3);
}

/// Poll three futures concurrently, in order — `Promise.all([1, 2, 3].map(...))`.
async fn futures_join3<A, B, C>(a: A, b: B, c: C)
where
    A: std::future::Future,
    B: std::future::Future,
    C: std::future::Future,
{
    tokio::join!(a, b, c);
}

/// The exclusive queue is FIFO: three concurrent calls to a sequential tool start and end in the
/// order they were made.
#[tokio::test]
async fn the_exclusive_queue_runs_calls_in_arrival_order() {
    let order = Arc::new(Mutex::new(Vec::<usize>::new()));
    let o = order.clone();
    let tool = arc(FnTool::new("seq", move |call_id, _| {
        let o = o.clone();
        Box::pin(async move {
            let n: usize = call_id
                .to_string()
                .rsplit('/')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            // Earlier calls sleep longer: only the queue keeps them in order.
            tokio::time::sleep(Duration::from_millis(20_u64.saturating_sub(n as u64 * 5))).await;
            o.lock().unwrap().push(n);
            Ok(ToolResult::default())
        })
    })
    .sequential());
    let host = TestHost::new(vec![tool], false);
    let runner = NestedToolCallRunner::new();
    let call = id("call");
    let run = || {
        runner.execute(
            &*host,
            &call,
            "seq",
            json!({}),
            NestedToolCallOptions::default(),
        )
    };
    tokio::join!(run(), run(), run());
    assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
}

/// A sequential tool that itself calls a sequential tool does not wait on the queue its own call
/// holds (`holdsQueue`): without the rule this never finishes.
#[tokio::test]
async fn a_sequential_call_made_inside_a_sequential_call_does_not_deadlock() {
    let leaf =
        arc(FnTool::new("leaf", |_, _| Box::pin(async { Ok(ToolResult::default()) })).sequential());
    let host = TestHost::new(vec![leaf], false);
    let runner = Arc::new(NestedToolCallRunner::new());
    let (h, r) = (host.clone(), runner.clone());
    host.push(arc(FnTool::new("outer", move |call_id, _| {
        let (h, r) = (h.clone(), r.clone());
        Box::pin(async move {
            r.execute(
                &*h,
                &call_id,
                "leaf",
                json!({}),
                NestedToolCallOptions::default(),
            )
            .await;
            r.execute(
                &*h,
                &call_id,
                "leaf",
                json!({}),
                NestedToolCallOptions::default(),
            )
            .await;
            Ok(ToolResult::default())
        })
    })
    .sequential()));

    let done = tokio::time::timeout(
        Duration::from_secs(5),
        runner.execute(
            &*host,
            &id("call"),
            "outer",
            json!({}),
            NestedToolCallOptions::default(),
        ),
    )
    .await;
    assert!(
        done.is_ok(),
        "nested-in-nested deadlocked on the exclusive queue"
    );
    let ids: Vec<_> = runner
        .take_record(&id("call"))
        .unwrap()
        .calls
        .unwrap()
        .calls
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(
        ids,
        vec![
            "call/1".to_string(),
            "call/1/1".to_string(),
            "call/1/2".to_string()
        ]
    );
}

/// With the whole agent sequential, every nested call is exclusive — including calls nested inside
/// calls — and still nothing deadlocks.
#[tokio::test]
async fn a_sequential_agent_runs_nested_calls_exclusively_without_deadlock() {
    let (active, max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let leaf = counting_tool("leaf", false, active, max.clone());
    let host = TestHost::new(vec![leaf], true);
    let runner = Arc::new(NestedToolCallRunner::new());
    let (h, r) = (host.clone(), runner.clone());
    host.push(arc(FnTool::new("outer", move |call_id, _| {
        let (h, r) = (h.clone(), r.clone());
        Box::pin(async move {
            r.execute(
                &*h,
                &call_id,
                "leaf",
                json!({}),
                NestedToolCallOptions::default(),
            )
            .await;
            Ok(ToolResult::default())
        })
    })));

    let (a, b, c) = (id("a"), id("b"), id("c"));
    let done = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            run_nested(&runner, &host, &a, "outer"),
            run_nested(&runner, &host, &b, "outer"),
            run_nested(&runner, &host, &c, "outer"),
        );
    })
    .await;
    assert!(done.is_ok(), "sequential agent deadlocked");
    assert_eq!(max.load(Ordering::SeqCst), 1, "nested calls overlapped");
}

/// The exclusive queue is released when a call's future is dropped mid-run.
#[tokio::test]
async fn a_dropped_call_releases_the_exclusive_queue() {
    let hang = Arc::new(OnceLock::<()>::new());
    let h = hang.clone();
    let slow = arc(FnTool::new("slow", move |_, _| {
        let h = h.clone();
        Box::pin(async move {
            let _ = h.set(());
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(ToolResult::default())
        })
    })
    .sequential());
    let quick = arc(FnTool::new("quick", |_, _| {
        Box::pin(async { Ok(ToolResult::default()) })
    })
    .sequential());
    let host = TestHost::new(vec![slow, quick], false);
    let runner = NestedToolCallRunner::new();

    let call = id("call");
    let first = runner.execute(
        &*host,
        &call,
        "slow",
        json!({}),
        NestedToolCallOptions::default(),
    );
    let _ = tokio::time::timeout(Duration::from_millis(50), first).await;
    assert!(hang.get().is_some(), "the slow call never started");

    let second = tokio::time::timeout(
        Duration::from_secs(5),
        runner.execute(
            &*host,
            &call,
            "quick",
            json!({}),
            NestedToolCallOptions::default(),
        ),
    )
    .await;
    assert!(
        second.is_ok(),
        "the queue stayed held after its call was dropped"
    );
}

/// Arguments that are not a JSON object come back as an error outcome, never a panic, and the call
/// is still recorded.
#[tokio::test]
async fn non_object_arguments_settle_as_an_error_outcome() {
    // The tool exists and would succeed: only the arguments make the call fail.
    let ran = Arc::new(AtomicUsize::new(0));
    let r = ran.clone();
    let echo = arc(FnTool::new("echo", move |_, _| {
        let r = r.clone();
        Box::pin(async move {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::default())
        })
    }));
    let host = TestHost::new(vec![echo], false);
    let runner = NestedToolCallRunner::new();
    let outcome = runner
        .execute(
            &*host,
            &id("call"),
            "echo",
            json!([1, 2]),
            NestedToolCallOptions::default(),
        )
        .await;
    assert!(outcome.is_error);
    assert_eq!(
        text_of(&outcome.result.content),
        "Invalid arguments for tool echo: expected an object, got array"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool ran");
    let record = runner.take_record(&id("call")).unwrap().calls.unwrap();
    assert_eq!(record.calls[0].status, NestedCallStatus::Error);
}

/// A nested event is a `tool_execution_*` event with `parentToolCallId` last; the loop's own
/// `tool_execution_*` events carry no such key (wire shape unchanged for existing consumers).
#[test]
fn nested_events_carry_the_parent_id_and_loop_events_do_not() {
    let nested = NestedToolExecutionEvent::ToolExecutionStart {
        tool_call_id: id("call/1"),
        tool_name: "read".into(),
        args: json!({ "path": "a" }),
        parent_tool_call_id: id("call"),
    };
    assert_eq!(
        serde_json::to_string(&nested).unwrap(),
        r#"{"type":"tool_execution_start","toolCallId":"call/1","toolName":"read","args":{"path":"a"},"parentToolCallId":"call"}"#
    );
    let end = NestedToolExecutionEvent::ToolExecutionEnd {
        tool_call_id: id("call/1"),
        tool_name: "read".into(),
        result: json!({ "content": [] }),
        is_error: false,
        parent_tool_call_id: id("call"),
    };
    assert_eq!(
        serde_json::to_string(&end).unwrap(),
        r#"{"type":"tool_execution_end","toolCallId":"call/1","toolName":"read","result":{"content":[]},"isError":false,"parentToolCallId":"call"}"#
    );

    let loop_events = [
        AgentEvent::ToolExecutionStart {
            tool_call_id: id("c"),
            tool_name: "read".into(),
            args: json!({}),
        },
        AgentEvent::ToolExecutionUpdate {
            tool_call_id: id("c"),
            tool_name: "read".into(),
            args: json!({}),
            partial_result: json!({}),
        },
        AgentEvent::ToolExecutionEnd {
            tool_call_id: id("c"),
            tool_name: "read".into(),
            result: json!({}),
            is_error: false,
        },
    ];
    for event in loop_events {
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("parentToolCallId"), "{json}");
    }
}

// ---------------------------------------------------------------------------------------------
// The hooks of a nested call: `run_tool_call` with a parent
// ---------------------------------------------------------------------------------------------

/// Records which entry point each hook was reached through, and with what parent.
#[derive(Default)]
struct RoutingHooks {
    seen: Mutex<Vec<(&'static str, Option<String>)>>,
}

#[async_trait::async_trait]
impl crate::Hooks for RoutingHooks {
    async fn before_tool_call(
        &self,
        _ctx: crate::hooks::BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> crate::hooks::BeforeOutcome {
        self.seen.lock().unwrap().push(("before", None));
        crate::hooks::BeforeOutcome::Proceed
    }
    async fn before_nested_tool_call(
        &self,
        parent: &ToolCallId,
        _ctx: crate::hooks::BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> crate::hooks::BeforeOutcome {
        self.seen
            .lock()
            .unwrap()
            .push(("before", Some(parent.to_string())));
        crate::hooks::BeforeOutcome::Proceed
    }
    async fn after_tool_call(
        &self,
        _ctx: crate::hooks::AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> crate::hooks::AfterOutcome {
        self.seen.lock().unwrap().push(("after", None));
        crate::hooks::AfterOutcome::Keep
    }
    async fn after_nested_tool_call(
        &self,
        parent: &ToolCallId,
        _ctx: crate::hooks::AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> crate::hooks::AfterOutcome {
        self.seen
            .lock()
            .unwrap()
            .push(("after", Some(parent.to_string())));
        crate::hooks::AfterOutcome::Keep
    }
}

/// Blocks every call through the ORDINARY entry point only — a hook written before nested calls
/// existed. The nested entry points' default must still reach it.
struct LegacyGate;

#[async_trait::async_trait]
impl crate::Hooks for LegacyGate {
    async fn before_tool_call(
        &self,
        _ctx: crate::hooks::BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> crate::hooks::BeforeOutcome {
        crate::hooks::BeforeOutcome::Block {
            reason: Some("legacy gate says no".to_string()),
            terminate: cyrup_core::TerminateHint::Unspecified,
        }
    }
}

fn assistant() -> cyrup_core::AssistantMessage {
    cyrup_provider::faux::faux_assistant_message(
        vec![Content::text("calling")],
        cyrup_core::StopReason::ToolUse,
    )
}

async fn run_with_parent(
    parent: Option<&ToolCallId>,
    hooks: &dyn crate::Hooks,
    tools: &[Arc<dyn Tool>],
) -> ToolCallOutcome {
    let mut arguments = serde_json::Map::new();
    arguments.insert("a".to_string(), json!(1));
    let call = ToolCall {
        id: id("call/1"),
        name: "echo".to_string(),
        arguments: arguments.into(),
        thought_signature: None,
        namespace: None,
    };
    let assistant = assistant();
    crate::run_tool_call(
        call,
        crate::RunToolCallOptions {
            tools,
            assistant_message: &assistant,
            context: crate::hooks::AgentContextView {
                system_prompt: "",
                messages: &[],
                tools: &[],
            },
            hooks,
            cancel: None,
            on_update: None,
            parent_tool_call_id: parent,
        },
    )
    .await
}

/// With a parent, `before_tool_call` and `after_tool_call` are reached through the nested entry
/// points and told the parent; without one, through the ordinary ones.
#[tokio::test]
async fn run_tool_call_with_a_parent_reaches_the_nested_hook_entry_points() {
    let tools: Vec<Arc<dyn Tool>> = vec![super::support::EchoTool::named("echo")];
    let hooks = RoutingHooks::default();

    let parent = id("call");
    let nested = run_with_parent(Some(&parent), &hooks, &tools).await;
    assert!(!nested.is_error);
    assert_eq!(
        *hooks.seen.lock().unwrap(),
        vec![
            ("before", Some("call".to_string())),
            ("after", Some("call".to_string()))
        ]
    );

    hooks.seen.lock().unwrap().clear();
    let plain = run_with_parent(None, &hooks, &tools).await;
    assert!(!plain.is_error);
    assert_eq!(
        *hooks.seen.lock().unwrap(),
        vec![("before", None), ("after", None)]
    );
}

/// A hook that never heard of nested calls still gates them: the nested entry points default to the
/// ordinary ones, so "a permission hook cannot be routed around by a tool that calls a tool" holds
/// for every `Hooks` implementation.
#[tokio::test]
async fn a_hook_that_does_not_know_about_nested_calls_still_blocks_them() {
    let tools: Vec<Arc<dyn Tool>> = vec![super::support::EchoTool::named("echo")];
    let parent = id("call");
    let outcome = run_with_parent(Some(&parent), &LegacyGate, &tools).await;
    assert!(outcome.is_error);
    assert_eq!(text_of(&outcome.result.content), "legacy gate says no");
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

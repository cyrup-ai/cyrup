//! AGENT-042 — a parallel batch does not start a call that was prepared before an abort.
//!
//! pi v0.85.0 (`afda4d620`, "fix(agent): stop prepared tools after preflight abort (#8936)") opens
//! every deferred closure in `executeToolCallsParallel` with
//! `if (signal?.aborted) { … createErrorToolResult("Operation aborted"), isError: true …;
//! await emitToolExecutionEnd(finalized, emit); return finalized; }`
//! (`packages/agent/src/agent-loop.ts:617-625` @v0.87.1): the tool is never executed, only
//! `tool_execution_end` is emitted for it, and `finalizeExecutedToolCall` — `afterToolCall` — is
//! never reached. The check runs when `Promise.all(finalizedCalls.map(…))` (`:643`) invokes the
//! closure, i.e. after the previous closure reached its first suspension point.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::{
    AfterOutcome, AfterToolCall, Agent, AgentEvent, BeforeOutcome, BeforeToolCall, Hooks,
    ToolExecution,
};
use cyrup_core::{
    CancelToken, Content, StopReason, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use serde_json::{Value, json};

use super::support::*;

/// Records that its body ran; optionally aborts the run in its synchronous prefix (before its
/// first suspension point) the way a listener reacting to the call could.
struct RecordingTool {
    name: String,
    params: Value,
    ran: Arc<AtomicUsize>,
    abort_on_start: Option<Arc<Mutex<Weak<Agent>>>>,
}

impl RecordingTool {
    fn new(name: &str) -> (Arc<Self>, Arc<AtomicUsize>) {
        Self::build(name, None)
    }

    fn aborting(name: &str, agent: Arc<Mutex<Weak<Agent>>>) -> (Arc<Self>, Arc<AtomicUsize>) {
        Self::build(name, Some(agent))
    }

    fn build(
        name: &str,
        abort_on_start: Option<Arc<Mutex<Weak<Agent>>>>,
    ) -> (Arc<Self>, Arc<AtomicUsize>) {
        let ran = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                name: name.into(),
                params: obj_schema(),
                ran: ran.clone(),
                abort_on_start,
            }),
            ran,
        )
    }
}

#[async_trait::async_trait]
impl Tool for RecordingTool {
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
        self.ran.fetch_add(1, Ordering::SeqCst);
        if let Some(agent) = self
            .abort_on_start
            .as_ref()
            .and_then(|a| a.lock().unwrap().upgrade())
        {
            agent.abort();
        }
        tokio::task::yield_now().await;
        Ok(ToolResult {
            content: vec![Content::text(format!("{} ran", self.name))],
            ..Default::default()
        })
    }
}

/// Aborts the run from inside `before_tool_call` for the named call — the user pressing Escape at
/// that call's permission dialog — and records every `after_tool_call` it sees.
struct AbortAtBefore {
    agent: Arc<Mutex<Weak<Agent>>>,
    abort_for: Option<&'static str>,
    after_calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Hooks for AbortAtBefore {
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> BeforeOutcome {
        if Some(ctx.tool_name) == self.abort_for
            && let Some(agent) = self.agent.lock().unwrap().upgrade()
        {
            agent.abort();
        }
        BeforeOutcome::Proceed
    }

    async fn after_tool_call(&self, ctx: AfterToolCall<'_>, _cancel: CancelToken) -> AfterOutcome {
        self.after_calls
            .lock()
            .unwrap()
            .push(ctx.tool_name.to_string());
        AfterOutcome::Keep
    }
}

fn two_call_batch() -> Arc<dyn crate::StreamFn> {
    faux_stream_fn(vec![
        faux_assistant_message(
            vec![
                faux_tool_call("a", json!({})),
                faux_tool_call("b", json!({})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ])
    .1
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The events recorded for one call, by tool name, as bare kind names.
fn events_for(rec: &EventRecorder, tool: &str) -> Vec<String> {
    rec.snapshot()
        .iter()
        .filter(|e| match e {
            AgentEvent::ToolExecutionStart { tool_name, .. }
            | AgentEvent::ToolExecutionUpdate { tool_name, .. }
            | AgentEvent::ToolExecutionEnd { tool_name, .. } => tool_name == tool,
            _ => false,
        })
        .map(ev_kind)
        .collect()
}

/// Call `a` is approved; the run is aborted while call `b`'s `before_tool_call` is open. pi v0.85+
/// runs neither: `b` settles through `prepareToolCall`'s post-hook abort check, `a` through the
/// deferred closure's new guard.
#[tokio::test]
async fn agent042_call_prepared_before_an_abort_is_finalized_unexecuted() {
    let slot = Arc::new(Mutex::new(Weak::new()));
    let (a, a_ran) = RecordingTool::new("a");
    let (b, b_ran) = RecordingTool::new("b");
    let hooks = Arc::new(AbortAtBefore {
        agent: slot.clone(),
        abort_for: Some("b"),
        after_calls: Mutex::new(Vec::new()),
    });
    let agent = Arc::new(
        Agent::builder(model_ref(), two_call_batch())
            .tools(vec![a, b])
            .tool_execution(ToolExecution::Parallel)
            .hooks(hooks.clone())
            .build(),
    );
    *slot.lock().unwrap() = Arc::downgrade(&agent);
    let rec = Arc::new(EventRecorder::default());
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(
        a_ran.load(Ordering::SeqCst),
        0,
        "a call prepared before the abort must not be executed (agent-loop.ts:617-625 @v0.87.1)"
    );
    assert_eq!(b_ran.load(Ordering::SeqCst), 0);
    assert!(
        hooks.after_calls.lock().unwrap().is_empty(),
        "the aborted closure returns before `finalizeExecutedToolCall`: no `afterToolCall`"
    );

    let msgs = rec.tool_result_messages();
    assert_eq!(msgs.len(), 2, "one tool result per call, in source order");
    for (msg, name) in msgs.iter().zip(["a", "b"]) {
        assert_eq!(&*msg.tool_name, name);
        assert_eq!(text_of(&msg.content), "Operation aborted");
        assert!(msg.is_error);
    }

    // The skipped call emits `tool_execution_end` only — no update — carrying
    // `createErrorToolResult`'s `{ content, details: {} }` with no `terminate` key.
    assert_eq!(
        events_for(&rec, "a"),
        ["tool_execution_start", "tool_execution_end"]
    );
    let end = rec
        .snapshot()
        .into_iter()
        .find_map(|e| match e {
            AgentEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } if &*tool_name == "a" => Some((result, is_error)),
            _ => None,
        })
        .unwrap();
    assert!(end.1, "isError: true");
    assert_eq!(end.0["content"][0]["text"], "Operation aborted");
    assert_eq!(end.0["details"], json!({}));
    assert!(end.0.get("terminate").is_none());
}

/// pi checks the signal when `map` invokes each closure, so an abort raised while call `a` runs to
/// its first suspension point stops call `b` from starting; `a` itself settles normally, with its
/// `afterToolCall`.
#[tokio::test]
async fn agent042_abort_while_an_earlier_call_starts_skips_the_later_call() {
    let slot = Arc::new(Mutex::new(Weak::new()));
    let (a, a_ran) = RecordingTool::aborting("a", slot.clone());
    let (b, b_ran) = RecordingTool::new("b");
    let hooks = Arc::new(AbortAtBefore {
        agent: slot.clone(),
        abort_for: None,
        after_calls: Mutex::new(Vec::new()),
    });
    let agent = Arc::new(
        Agent::builder(model_ref(), two_call_batch())
            .tools(vec![a, b])
            .tool_execution(ToolExecution::Parallel)
            .hooks(hooks.clone())
            .build(),
    );
    *slot.lock().unwrap() = Arc::downgrade(&agent);
    let rec = Arc::new(EventRecorder::default());
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(a_ran.load(Ordering::SeqCst), 1);
    assert_eq!(
        b_ran.load(Ordering::SeqCst),
        0,
        "b's closure sees the signal a aborted before b's turn to start"
    );
    assert_eq!(*hooks.after_calls.lock().unwrap(), ["a"]);

    let msgs = rec.tool_result_messages();
    assert_eq!(msgs.len(), 2);
    assert_eq!(text_of(&msgs[0].content), "a ran");
    assert!(!msgs[0].is_error);
    assert_eq!(text_of(&msgs[1].content), "Operation aborted");
    assert!(msgs[1].is_error);
    assert_eq!(
        events_for(&rec, "b"),
        ["tool_execution_start", "tool_execution_end"]
    );
}

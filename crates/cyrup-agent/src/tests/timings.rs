//! Response and tool-execution times — pi commit 36a686ee8 ("record response, tool execution, and
//! durable task times", #10549), shipped in v1.1.0.
//!
//! - `ToolResultMessage.durationMs` is how long `execute()` took, measured with a monotonic clock
//!   around the call alone (`executePreparedToolCall`, `packages/agent/src/agent-loop.ts:828-846`
//!   @v1.1.0): hooks are outside it, and a call that never ran has no key at all.
//! - `tool_execution_end` and the programmatic `AgentToolCallOutcome` carry the same value.
//! - The settled assistant message carries `durationMs` from its response's stream
//!   (`AssistantMessageEventStream`, `packages/ai/src/utils/event-stream.ts` @v1.1.0) — including
//!   the aborted message an abort settles, which pi's stream times as the abort's `error` event.

use std::sync::Arc;
use std::time::Duration;

use super::support::*;
use crate::hooks::{AgentContextView, BeforeOutcome, BeforeToolCall, DefaultHooks};
use crate::{
    Agent, AgentEvent, AgentMessage, Hooks, RunToolCallOptions, StreamFn, ToolResultMessage,
    run_tool_call,
};
use cyrup_core::{
    CancelToken, Content, EventStream, ModelRef, StopReason, TerminateHint, Tool, ToolCall,
    ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use cyrup_provider::{Context, StreamEvent, StreamOptions};
use serde_json::{Value, json};

fn now_ms() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
}

/// A tool whose `execute()` takes `ms` milliseconds.
struct SlowTool {
    name: &'static str,
    ms: u64,
    params: Value,
}

impl SlowTool {
    fn new(name: &'static str, ms: u64) -> Arc<Self> {
        Arc::new(Self {
            name,
            ms,
            params: obj_schema(),
        })
    }
}

#[async_trait::async_trait]
impl Tool for SlowTool {
    fn name(&self) -> &str {
        self.name
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
        tokio::time::sleep(Duration::from_millis(self.ms)).await;
        Ok(ToolResult {
            content: vec![Content::text("ok")],
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

/// pi's test hook: every `before_tool_call` sleeps 100 ms — time that must NOT count — and the
/// `gate` tool is blocked.
struct SlowGate;

#[async_trait::async_trait]
impl Hooks for SlowGate {
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> BeforeOutcome {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if ctx.tool_name == "gate" {
            BeforeOutcome::Block {
                reason: Some("no".into()),
                terminate: TerminateHint::Unspecified,
            }
        } else {
            BeforeOutcome::Proceed
        }
    }
}

/// pi `agent-loop.test.ts` "records how long execute() took on the tool result, excluding hooks"
/// (#10549), with the blocked call told apart by tool name rather than call id.
#[tokio::test]
async fn a_tool_result_records_how_long_execute_took_excluding_hooks() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(
            vec![
                faux_tool_call("slow", json!({})),
                faux_tool_call("gate", json!({})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ])
    .1;
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![SlowTool::new("slow", 30), SlowTool::new("gate", 30)])
        .hooks(Arc::new(SlowGate))
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();
    let results = first_turn_results(&events);
    let ran = results.iter().find(|r| r.tool_name == "slow").unwrap();
    let blocked = results.iter().find(|r| r.tool_name == "gate").unwrap();
    let took = ran.duration_ms.expect("the call that ran has a duration");
    assert!((25..100).contains(&took), "execute() took {took} ms");
    assert!(blocked.is_error);
    assert_eq!(blocked.duration_ms, None, "a blocked call did not run");

    // The persisted shape: the key sits between `isError` and `timestamp`, and is absent for the
    // blocked call.
    let wire = serde_json::to_string(ran).unwrap();
    let at = |k: &str| wire.find(k).unwrap_or_else(|| panic!("{k} in {wire}"));
    assert!(at("\"isError\"") < at("\"durationMs\""), "{wire}");
    assert!(at("\"durationMs\"") < at("\"timestamp\""), "{wire}");
    assert!(
        !serde_json::to_string(blocked)
            .unwrap()
            .contains("durationMs")
    );

    // `tool_execution_end` carries the same value (pi `emitToolExecutionEnd`, `:920-927`).
    let ends: Vec<(String, Option<u64>)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd {
                tool_name,
                duration_ms,
                ..
            } => Some((tool_name.clone(), *duration_ms)),
            _ => None,
        })
        .collect();
    assert!(ends.contains(&("slow".to_string(), Some(took))), "{ends:?}");
    assert!(ends.contains(&("gate".to_string(), None)), "{ends:?}");
}

/// A thrown tool still has a duration: pi measures on the catch path too (`:843-846`).
#[tokio::test]
async fn a_tool_that_throws_still_records_how_long_it_ran() {
    struct Throws;
    #[async_trait::async_trait]
    impl Tool for Throws {
        fn name(&self) -> &str {
            "throws"
        }
        fn parameters(&self) -> &Value {
            static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(obj_schema)
        }
        async fn execute(
            &self,
            _call_id: ToolCallId,
            _params: Value,
            _cancel: CancelToken,
            _on_update: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Err(ToolError::new("boom"))
        }
    }
    let sf = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("throws", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ])
    .1;
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![Arc::new(Throws)])
        .build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let results: Vec<ToolResultMessage> = first_turn_results(&rec.snapshot());
    assert!(results[0].is_error);
    let took = results[0].duration_ms.expect("a throw is still timed");
    assert!(took >= 25, "{took}");
}

/// `run_tool_call` — pi's `AgentToolCallOutcome.durationMs?` — carries it, and its wire object
/// writes the key last; a call refused before it ran has none.
#[tokio::test]
async fn the_programmatic_outcome_carries_the_duration() {
    let tools: Vec<Arc<dyn Tool>> = vec![SlowTool::new("slow", 30)];
    let assistant = faux_assistant_message(vec![faux_text("calling")], StopReason::ToolUse);
    let run = |name: &str| {
        let call = ToolCall {
            id: ToolCallId::from("c1"),
            name: name.to_string(),
            arguments: serde_json::Map::new().into(),
            thought_signature: None,
            namespace: None,
        };
        let tools = &tools;
        let assistant = &assistant;
        async move {
            run_tool_call(
                call,
                RunToolCallOptions {
                    tools,
                    assistant_message: assistant,
                    context: AgentContextView {
                        system_prompt: "sys",
                        messages: &[],
                        tools,
                    },
                    hooks: &DefaultHooks,
                    cancel: None,
                    on_update: None,
                    parent_tool_call_id: None,
                },
            )
            .await
        }
    };
    let ran = run("slow").await;
    let took = ran.duration_ms.expect("timed");
    assert!(took >= 25, "{took}");
    let wire = ran.to_wire();
    assert_eq!(wire["durationMs"], json!(took));
    let keys: Vec<&String> = wire.as_object().unwrap().keys().collect();
    assert_eq!(
        keys.last().map(|k| k.as_str()),
        Some("durationMs"),
        "{wire}"
    );

    let unknown = run("missing").await;
    assert!(unknown.is_error);
    assert_eq!(unknown.duration_ms, None);
    assert!(unknown.to_wire().get("durationMs").is_none());
}

/// The settled assistant message carries its response's duration, and `message_start` (owed on a
/// stream that never sent `start`) and `message_end` publish the same timed message.
#[tokio::test]
async fn the_settled_assistant_message_carries_its_responses_duration() {
    /// A stream function whose only event is a `done`, 30 ms after the request.
    struct LateDone;
    impl StreamFn for LateDone {
        fn stream(
            &self,
            _m: &ModelRef,
            _c: &Context,
            _o: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            Box::pin(futures::stream::once(async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                let mut m = faux_assistant_message(vec![faux_text("hi")], StopReason::Stop);
                m.timestamp = now_ms();
                StreamEvent::terminal(m)
            }))
        }
    }
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), Arc::new(LateDone)).build();
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let events = rec.snapshot();
    let settled = last_assistant(&events);
    let took = settled.duration_ms.expect("the settled message is timed");
    assert!(took >= 25, "{took}");
    let started: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageStart {
                message: AgentMessage::Assistant(a),
            } => Some(a.duration_ms),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![Some(took)]);
}

/// An abort mid-response settles an `aborted` message here, in the loop; it is timed from the
/// request as pi's stream times the abort's `error` event. Its `timestamp` is the request's start
/// even though no `start` event ever arrived to carry one.
#[tokio::test]
async fn an_aborted_response_is_timed_from_the_request() {
    struct Hangs;
    impl StreamFn for Hangs {
        fn stream(
            &self,
            _m: &ModelRef,
            _c: &Context,
            _o: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            Box::pin(futures::stream::pending())
        }
    }
    let rec = Arc::new(EventRecorder::default());
    let agent = Arc::new(Agent::builder(model_ref(), Arc::new(Hangs)).build());
    agent.subscribe(rec.clone());
    let before = now_ms();
    let run = agent.prompt("go").await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    agent.abort();
    run.finished().await;
    agent.wait_for_idle().await;

    let settled = last_assistant(&rec.snapshot());
    assert_eq!(settled.stop_reason, StopReason::Aborted);
    let took = settled.duration_ms.expect("an abort is timed");
    assert!(took >= 35, "{took}");
    assert!(
        settled.timestamp >= before,
        "{} < {before}",
        settled.timestamp
    );
}

//! Preflight tool-argument validation + coercion (func-02 R-02-020 / func-01 R-01-034), driven by
//! the faux provider and a tool whose schema requires `{ "n": integer }`.

use std::sync::{Arc, Mutex};

use crate::{Agent, AgentMessage};
use cyrup_core::{
    TerminateHint,
    CancelToken, Content, StopReason, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use serde_json::{json, Value};

use super::support::*;

/// A tool requiring `{ "n": integer }` that records every params object it actually executes with.
struct IntTool {
    params: Value,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl IntTool {
    fn new() -> (Arc<Self>, Arc<Mutex<Vec<Value>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
            "required": ["n"],
        });
        (Arc::new(Self { params: schema, seen: seen.clone() }), seen)
    }
}

#[async_trait::async_trait]
impl Tool for IntTool {
    fn name(&self) -> &str {
        "int_tool"
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
        self.seen.lock().unwrap().push(params.clone());
        Ok(ToolResult {
            content: vec![Content::text(format!("got:{params}"))],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

fn tool_result_errors(msgs: &[AgentMessage]) -> Vec<&crate::ToolResultMessage> {
    msgs.iter()
        .filter_map(|m| match m {
            AgentMessage::ToolResult(t) if t.is_error => Some(t),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn valid_args_execute_normally() {
    let (tool, seen) = IntTool::new();
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("int_tool", json!({ "n": 7 }))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();

    let new = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let executed = seen.lock().unwrap().clone();
    assert_eq!(executed, vec![json!({ "n": 7 })], "tool executed with valid args");
    assert!(tool_result_errors(&new).is_empty(), "no error tool-results: {new:?}");
}

#[tokio::test]
async fn coercible_args_are_coerced_and_reach_execute() {
    let (tool, seen) = IntTool::new();
    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("int_tool", json!({ "n": "123" }))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let executed = seen.lock().unwrap().clone();
    // "123" coerced to the integer 123 before reaching execute.
    assert_eq!(executed, vec![json!({ "n": 123 })], "string arg coerced to integer");
}

#[tokio::test]
async fn invalid_args_yield_error_result_seen_next_turn_without_executing() {
    let (tool, seen) = IntTool::new();
    let (faux, sf) = faux_stream_fn(vec![
        // uncoercible: "abc" cannot become an integer.
        faux_assistant_message(
            vec![faux_tool_call("int_tool", json!({ "n": "abc" }))],
            StopReason::ToolUse,
        ),
        // the loop must continue and stream a second assistant turn (where the model SEES the
        // isError tool-result in history).
        faux_assistant_message(vec![faux_text("ok, retrying")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();

    let new = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // Tool NOT executed.
    assert!(seen.lock().unwrap().is_empty(), "uncoercible args must not execute the tool");

    // An isError tool-result was produced.
    let errs = tool_result_errors(&new);
    assert_eq!(errs.len(), 1, "exactly one isError tool-result: {new:?}");

    // The loop continued: the model was streamed a second turn (so it sees the error result).
    assert_eq!(faux.call_count(), 2, "loop continued into a second assistant turn");
    assert!(
        matches!(new.last(), Some(AgentMessage::Assistant(_))),
        "run ends on a fresh assistant turn after the error: {new:?}"
    );
}

#[tokio::test]
async fn missing_required_field_yields_error_result_without_executing() {
    let (tool, seen) = IntTool::new();
    let (faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("int_tool", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();

    let new = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert!(seen.lock().unwrap().is_empty(), "missing-required must not execute the tool");
    let errs = tool_result_errors(&new);
    assert_eq!(errs.len(), 1, "missing required field yields one isError result");
    assert!(
        errs[0].content.iter().any(|c| matches!(c, Content::Text { text, .. } if text.contains("required"))),
        "error message mentions the missing required field: {:?}",
        errs[0].content
    );
    assert_eq!(faux.call_count(), 2, "loop continues after the error result");
}

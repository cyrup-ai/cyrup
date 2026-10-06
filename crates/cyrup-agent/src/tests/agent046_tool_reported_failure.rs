//! AGENT-046 — a tool reports failure WITHOUT returning `Err`, and keeps its data.
//!
//! pi `AgentToolResult.isError?` (`packages/agent/src/types.ts:436-440` @v1.0.1): *"Report a
//! failure without throwing. The model sees `content` as an error result, like a thrown error, but
//! `details` and `structuredContent` are kept for the UI and programmatic callers."* Honoured by
//! one line — `executePreparedToolCall`'s success return at `agent-loop.ts:840`, which changed from
//! `{ result, isError: false }` to `{ result, isError: result.isError === true }`.
//!
//! The whole point is the ASYMMETRY against `Err`, so every assertion here is paired with the same
//! tool failing the other way.

use std::sync::Arc;

use super::support::{EventRecorder, faux_stream_fn, model_ref, obj_schema};
use crate::hooks::{AfterOutcome, AfterOverride, AfterToolCall, AgentContextView, DefaultHooks};
use crate::{Agent, Hooks, RunToolCallOptions, ToolCallOutcome, run_tool_call};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, StopReason, TerminateHint, Tool, ToolCall, ToolCallId,
    ToolError, ToolResult, ToolUpdateSink, Usage,
};
use cyrup_provider::faux::{faux_assistant_message, faux_tool_call};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// How the tool reports its failure.
#[derive(Clone, Copy)]
enum Mode {
    /// `Ok` + `is_error: true`, carrying every optional half.
    ReportedOk,
    /// `Err(ToolError::with_details(..))` — cyrup's primary failure path.
    ErrWithDetails,
}

fn payload() -> Value {
    json!({ "exitCode": 2, "stderr": "no such file" })
}

fn detail() -> Value {
    json!({ "attempted": "cat /nope" })
}

struct ReportingTool {
    params: Value,
    mode: Mode,
}

impl ReportingTool {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            params: obj_schema(),
            mode,
        })
    }
}

#[async_trait::async_trait]
impl Tool for ReportingTool {
    fn name(&self) -> &str {
        "reporter"
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
        match self.mode {
            Mode::ReportedOk => Ok(ToolResult {
                content: vec![Content::text("cat: /nope: No such file")],
                details: Some(detail()),
                usage: Some(Usage {
                    input: 13,
                    ..Usage::default()
                }),
                structured_content: Some(payload()),
                is_error: true,
                terminate: TerminateHint::Unspecified,
                added_tool_names: Vec::new(),
            }),
            Mode::ErrWithDetails => {
                Err(ToolError::new("cat: /nope: No such file").with_details(detail()))
            }
        }
    }
}

fn assistant() -> AssistantMessage {
    faux_assistant_message(vec![Content::text("calling")], StopReason::ToolUse)
}

fn call() -> ToolCall {
    ToolCall {
        id: ToolCallId::from("c1"),
        name: "reporter".to_string(),
        arguments: serde_json::Map::new().into(),
        thought_signature: None,
        namespace: None,
    }
}

/// Run the tool once through the programmatic entry point (AGENT-047), which is the shortest path
/// that exercises the real `prepare`/`execute`/`finalize` pipeline.
async fn run(mode: Mode, hooks: &dyn Hooks) -> ToolCallOutcome {
    let tools: Vec<Arc<dyn Tool>> = vec![ReportingTool::new(mode)];
    run_tool_call(
        call(),
        RunToolCallOptions {
            tools: &tools,
            assistant_message: &assistant(),
            context: AgentContextView {
                system_prompt: "sys",
                messages: &[],
                tools: &tools,
            },
            hooks,
            cancel: None,
            on_update: None,
            parent_tool_call_id: None,
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// The asymmetry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent046_an_ok_result_that_declares_failure_is_an_error_to_the_loop() {
    let outcome = run(Mode::ReportedOk, &DefaultHooks).await;
    // pi `:840` — `isError: result.isError === true`. Without it the loop normalises every `Ok` to
    // `isError: false` and the model is told a failed call succeeded.
    assert!(
        outcome.is_error,
        "the loop's verdict follows the tool's own flag"
    );
    assert!(outcome.result.is_error, "the tool's flag rides through");
}

#[tokio::test]
async fn agent046_a_reported_failure_keeps_details_usage_structured_and_terminate() {
    let reported = run(Mode::ReportedOk, &DefaultHooks).await;
    assert_eq!(reported.result.details, Some(detail()));
    assert_eq!(
        reported.result.usage.as_ref().map(|u| u.input),
        Some(13),
        "pi: `usage` is KEPT on a reported failure"
    );
    assert_eq!(
        reported.result.structured_content,
        Some(payload()),
        "pi: `structuredContent` is KEPT on a reported failure"
    );

    // The same failure thrown instead: cyrup's port of `createErrorToolResult` replaces the whole
    // result. `details` survives only because `ToolError::details` exists (its [CYRUP-DELTA]);
    // `usage` and `structured_content` do not. That difference IS the field's reason to exist.
    let thrown = run(Mode::ErrWithDetails, &DefaultHooks).await;
    assert!(thrown.is_error);
    assert_eq!(
        thrown.result.details,
        Some(detail()),
        "`ToolError::with_details` already reaches the caller today — AGENT-046's CORRECTED note"
    );
    assert!(
        thrown.result.usage.is_none(),
        "pi's `createErrorToolResult` carries no usage"
    );
    assert!(
        thrown.result.structured_content.is_none(),
        "nor a structured half"
    );
    assert!(
        !thrown.result.is_error,
        "`createErrorToolResult` writes no `isError` key, so the RESULT object reports none even \
         though the OUTCOME is an error"
    );
}

/// An `after_tool_call` that clears the verdict.
struct Clearing;

#[async_trait::async_trait]
impl Hooks for Clearing {
    async fn after_tool_call(&self, ctx: AfterToolCall<'_>, _c: CancelToken) -> AfterOutcome {
        assert!(
            ctx.is_error,
            "the hook is SHOWN the tool's reported failure (pi `isError` in the hook context, \
             agent-loop.ts:872)"
        );
        AfterOutcome::Override(Box::new(AfterOverride {
            is_error: Some(false),
            ..Default::default()
        }))
    }
}

#[tokio::test]
async fn agent046_a_hook_flips_the_verdict_but_not_the_tools_own_flag() {
    let outcome = run(Mode::ReportedOk, &Clearing).await;
    // pi `isError = afterResult.isError ?? isError` (`:890`) — the VERDICT changes.
    assert!(!outcome.is_error, "the hook cleared the loop's verdict");
    // pi's `result = {...result, …}` never assigns `isError` (`:881-887`) — the tool's own flag
    // is untouched, exactly as `addedToolNames` is.
    assert!(
        outcome.result.is_error,
        "no hook can rewrite the tool's own flag"
    );
}

// ---------------------------------------------------------------------------
// What reaches the transcript and the event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent046_a_reported_failure_is_an_error_result_on_the_transcript_and_the_event() {
    let tools: Vec<Arc<dyn Tool>> = vec![ReportingTool::new(Mode::ReportedOk)];
    let responses = vec![
        faux_assistant_message(
            vec![faux_tool_call("reporter", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![Content::text("noted")], StopReason::Stop),
    ];
    let agent = Agent::builder(model_ref(), faux_stream_fn(responses).1)
        .system_prompt("sys")
        .tools(tools)
        .build();
    let recorder = Arc::new(EventRecorder::default());
    let _sub = agent.subscribe(recorder.clone());
    agent.prompt("go").await.expect("run starts");
    agent.wait_for_idle().await;

    let msgs = recorder.tool_result_messages();
    let msg = msgs.first().expect("a tool-result message");
    assert!(
        msg.is_error,
        "the transcript records an error result — the model reads it as a failure"
    );
    assert_eq!(
        msg.details,
        Some(detail()),
        "and `details` survives, which is what a thrown failure cannot promise"
    );
    assert_eq!(
        msg.usage.as_ref().map(|u| u.input),
        Some(13),
        "and so does `usage`"
    );

    let results = recorder.end_results();
    let result = results.first().expect("a tool_execution_end result");
    assert_eq!(
        result.get("isError"),
        Some(&json!(true)),
        "the tool's own flag reaches `tool_execution_end.result`, because pi emits \
         `finalized.result` verbatim"
    );
    assert_eq!(result.get("structuredContent"), Some(&payload()));
}

#[tokio::test]
async fn agent046_a_plain_ok_result_puts_no_is_error_key_on_the_wire() {
    // `false` is pi's ABSENT key. A `ToolResult` built the ordinary way must not start emitting it.
    let tools: Vec<Arc<dyn Tool>> = vec![super::support::EchoTool::named("echo")];
    let responses = vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![Content::text("noted")], StopReason::Stop),
    ];
    let agent = Agent::builder(model_ref(), faux_stream_fn(responses).1)
        .system_prompt("sys")
        .tools(tools)
        .build();
    let recorder = Arc::new(EventRecorder::default());
    let _sub = agent.subscribe(recorder.clone());
    agent.prompt("go").await.expect("run starts");
    agent.wait_for_idle().await;

    let results = recorder.end_results();
    let result = results.first().expect("a tool_execution_end result");
    assert!(
        result.get("isError").is_none(),
        "no `isError` key on a result that did not set one; got {result}"
    );
    let msgs = recorder.tool_result_messages();
    assert!(!msgs.first().expect("a tool-result message").is_error);
}

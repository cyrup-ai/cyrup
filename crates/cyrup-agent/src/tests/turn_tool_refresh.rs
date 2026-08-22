//! DRIFT-001, agent-loop half — `TurnUpdate::tools` / `TurnUpdate::system_prompt`.
//!
//! The loop snapshots its tool array ONCE at run start (Pi `createContextSnapshot`, agent.ts:424-429
//! → cyrup `Agent::start_run`), so nothing that happens during a run can change what the model may
//! call. Pi closes that with `_installAgentNextTurnRefresh` (coding-agent/src/core/agent-session.ts:
//! 519-540), whose `prepareNextTurnWithContext` returns `context: {...previousContext, systemPrompt,
//! tools: this.agent.state.tools.slice()}` on EVERY turn — `AgentContext` carries `tools`, so a tool
//! set change reaches the running loop through the same post-turn seam as a model or reasoning
//! change.
//!
//! cyrup's `TurnUpdate` is the analogue of that return value, and it modelled only
//! `context`/`model`/`thinking_level`. These tests pin the two new fields at the loop level: they
//! reach the very next provider request, they are STICKY like every other `TurnUpdate` field
//! (agent-loop.ts:226-239), and a tool introduced this way is genuinely EXECUTED — which is the
//! precondition a `ToolResult::added_tool_names` anchor asserts.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Agent, HookError, Hooks, PostTurn, StreamFn, TurnUpdate};
use cyrup_core::{
    CancelToken, Content, EventStream, ModelRef, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::{Context, StreamEvent, StreamOptions};
use serde_json::{json, Value};

use super::support::model_ref;

/// What the agent actually offered the model on one request: the tool names and the system prompt.
type Requests = Arc<Mutex<Vec<(Vec<String>, String)>>>;

struct ToolRequestSpy {
    inner: Arc<dyn StreamFn>,
    seen: Requests,
}

impl StreamFn for ToolRequestSpy {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.seen.lock().unwrap().push((
            ctx.tools.iter().map(|t| t.name.clone()).collect(),
            ctx.system_prompt.clone().unwrap_or_default(),
        ));
        self.inner.stream(model, ctx, opts)
    }
}

fn recording(responses: Vec<cyrup_core::AssistantMessage>) -> (Arc<dyn StreamFn>, Requests) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let seen: Requests = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> =
        Arc::new(ToolRequestSpy { inner: Arc::new(crate::ProviderStreamFn::new(faux)), seen: seen.clone() });
    (sf, seen)
}

/// A tool that records whether the loop ever dispatched to it.
struct FlagTool {
    name: String,
    ran: Arc<AtomicBool>,
    params: Value,
}

impl FlagTool {
    /// Not `new`: returns the tool AND its ran-flag, so a caller can assert execution.
    fn pair(name: &str) -> (Arc<dyn Tool>, Arc<AtomicBool>) {
        let ran = Arc::new(AtomicBool::new(false));
        let t: Arc<dyn Tool> = Arc::new(FlagTool {
            name: name.to_string(),
            ran: ran.clone(),
            params: json!({ "type": "object", "properties": {} }),
        });
        (t, ran)
    }
}

#[async_trait::async_trait]
impl Tool for FlagTool {
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
        self.ran.store(true, Ordering::SeqCst);
        Ok(ToolResult { content: vec![Content::text("ok")], ..Default::default() })
    }
}

/// Returns the widened tool set + a new system prompt exactly ONCE, after turn 1 — the shape Pi's
/// refresh produces the first time a tool becomes active mid-run.
struct RefreshOnce {
    turns: AtomicUsize,
    tools: Vec<Arc<dyn Tool>>,
}

#[async_trait::async_trait]
impl Hooks for RefreshOnce {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<Option<TurnUpdate>, HookError> {
        if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Some(TurnUpdate {
                tools: Some(self.tools.clone()),
                system_prompt: Some("REFRESHED".to_string()),
                ..TurnUpdate::default()
            }))
        } else {
            Ok(None)
        }
    }
}

/// A tool handed to the loop only by a `prepare_next_turn` refresh is offered on the NEXT request,
/// was NOT offered before it, is actually EXECUTED when the model calls it, and — like every other
/// `TurnUpdate` field — STICKS for the rest of the run.
#[tokio::test]
async fn turn_update_tools_reach_the_next_request_and_stick() {
    let (echo, _) = FlagTool::pair("echo");
    let (late, late_ran) = FlagTool::pair("late");

    // Turn 1 calls `echo`, turn 2 calls the newly-refreshed `late`, turn 3 calls `echo` again (so a
    // third request is built and the stickiness of the refresh is observable), turn 4 stops.
    let (sf, seen) = recording(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_tool_call("late", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);

    let agent = Agent::builder(model_ref(), sf)
        .system_prompt("BASE".to_string())
        .tools(vec![echo.clone()])
        .hooks(Arc::new(RefreshOnce {
            turns: AtomicUsize::new(0),
            tools: vec![echo, late],
        }))
        .build();

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 4, "four provider requests: {reqs:?}");

    // NOT BEFORE.
    assert_eq!(reqs[0].0, vec!["echo".to_string()], "turn 1 runs on the run-start snapshot");
    assert_eq!(reqs[0].1, "BASE", "turn 1 uses the run's system prompt");

    // FROM THE REFRESH ONWARD.
    assert!(reqs[1].0.contains(&"late".to_string()), "turn 2 offers the refreshed tool: {:?}", reqs[1].0);
    assert!(reqs[1].0.contains(&"echo".to_string()), "turn 2 kept the original tool: {:?}", reqs[1].0);
    assert_eq!(reqs[1].1, "REFRESHED", "turn 2 uses the refreshed system prompt");

    // STICKY — the hook returned the update ONCE, and turn 3 still runs on it (agent-loop.ts:226-239).
    assert!(reqs[2].0.contains(&"late".to_string()), "turn 3 keeps the refreshed tools: {:?}", reqs[2].0);
    assert_eq!(reqs[2].1, "REFRESHED", "turn 3 keeps the refreshed system prompt");

    // CALLABLE, not merely advertised.
    assert!(late_ran.load(Ordering::SeqCst), "the refreshed tool actually executed");
}

/// The absent case is a no-op: a `TurnUpdate` that leaves both fields `None` must not clear the
/// run's tools or blank its system prompt. (`Option::None` = "keep the current baseline", the same
/// contract `model`/`thinking_level` have.)
struct ModelOnlyUpdate;

#[async_trait::async_trait]
impl Hooks for ModelOnlyUpdate {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<Option<TurnUpdate>, HookError> {
        Ok(Some(TurnUpdate::default()))
    }
}

#[tokio::test]
async fn a_turn_update_without_tools_leaves_the_run_baseline_alone() {
    let (echo, _) = FlagTool::pair("echo");
    let (sf, seen) = recording(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .system_prompt("BASE".to_string())
        .tools(vec![echo])
        .hooks(Arc::new(ModelOnlyUpdate))
        .build();

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let reqs = seen.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert_eq!(reqs[1].0, vec!["echo".to_string()], "the tool set survived an empty update");
    assert_eq!(reqs[1].1, "BASE", "the system prompt survived an empty update");
}

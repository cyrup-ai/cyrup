//! Round-2 parity conformance for the four in-crate gaps closed against Pi `agent-loop.ts`/`agent.ts`:
//!
//! - Gap #3 — the low-level free-function loop (`run_agent_loop`/`agent_loop`/`*_continue` +
//!   `AgentEventSink`), Pi `agent-loop.ts:25,31-143`.
//! - Gap #4 — `prepare_next_turn` overrides are STICKY (new baseline for all later turns), Pi
//!   `agent-loop.ts:226-239`.
//! - Gap #5 — `terminate` still runs the post-turn path (hooks + steering/follow-up), Pi
//!   `agent-loop.ts:210,218-262`.
//! - Gap #6 — an uncontained run failure reports the real panic message + aborted-vs-error, Pi
//!   `agent.ts:496-511`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    agent_loop, agent_loop_continue, run_agent_loop, run_agent_loop_continue, AfterOverride,
    AfterToolCall, Agent, AgentContext, AgentEvent, AgentEventSink, AgentLoopConfig, AgentMessage,
    Hooks, HookError, PostTurn, StreamFn, TurnUpdate,
};
use cyrup_core::{
    CancelToken, Content, EventStream, ModelRef, ModelThinkingLevel, RunCancel, StopReason,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use cyrup_provider::{Context, StreamEvent, StreamOptions};
use futures::StreamExt;
use serde_json::json;

use super::support::*;

// ----------------------------------------------------------------------------
// Wiring
// ----------------------------------------------------------------------------

/// A `StreamFn` that records the reasoning level forwarded on each provider call, then delegates to
/// a faux provider for the event stream.
struct ReasoningRecorder {
    inner: Arc<dyn StreamFn>,
    reasoning: Arc<Mutex<Vec<ModelThinkingLevel>>>,
}

impl StreamFn for ReasoningRecorder {
    fn stream(&self, model: &ModelRef, ctx: &Context, opts: &StreamOptions) -> EventStream<StreamEvent> {
        self.reasoning.lock().unwrap().push(opts.reasoning);
        self.inner.stream(model, ctx, opts)
    }
}

fn reasoning_recording(
    responses: Vec<cyrup_core::AssistantMessage>,
) -> (Arc<dyn StreamFn>, Arc<Mutex<Vec<ModelThinkingLevel>>>) {
    let inner = faux_stream_fn(responses).1;
    let reasoning = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(ReasoningRecorder { inner, reasoning: reasoning.clone() });
    (sf, reasoning)
}

/// Records every event delivered by the loop, in order.
#[derive(Default)]
struct RecSink {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl AgentEventSink for RecSink {
    async fn emit(&self, event: &AgentEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

impl RecSink {
    fn names(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(ev_kind).collect()
    }
    fn turn_starts(&self) -> usize {
        self.events.lock().unwrap().iter().filter(|e| matches!(e, AgentEvent::TurnStart)).count()
    }
}

// ============================================================================
// Gap #4 — prepare_next_turn overrides are STICKY (agent-loop.ts:226-239).
// ============================================================================

/// Overrides `thinking_level` to `High` exactly ONCE (after the first turn), then returns `None`.
/// Under Pi's sticky semantics that High level persists for EVERY later turn; a one-shot (the old
/// cyrup behavior) would revert to the run default on the third turn.
struct StickyThinkingHook {
    overridden: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for StickyThinkingHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<Option<TurnUpdate>, HookError> {
        if self.overridden.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Some(TurnUpdate {
                thinking_level: Some(ModelThinkingLevel::High),
                ..TurnUpdate::default()
            }))
        } else {
            Ok(None)
        }
    }
}

#[tokio::test]
async fn gap4_prepare_next_turn_override_is_sticky_across_later_turns() {
    // Three provider requests: turn 1 + 2 each emit a tool call (forcing continuation), turn 3 stops.
    let (sf, reasoning) = reasoning_recording(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .thinking_level(ModelThinkingLevel::Low)
        .tools(vec![EchoTool::named("echo")])
        .hooks(Arc::new(StickyThinkingHook { overridden: AtomicUsize::new(0) }))
        .build();

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let r = reasoning.lock().unwrap().clone();
    assert_eq!(r.len(), 3, "three provider requests");
    assert_eq!(r[0], ModelThinkingLevel::Low, "turn 1 uses the run default");
    assert_eq!(r[1], ModelThinkingLevel::High, "turn 2 uses the override");
    // The crux: the override was returned ONCE, but Pi makes it sticky, so turn 3 is STILL High.
    assert_eq!(r[2], ModelThinkingLevel::High, "turn 3 keeps the sticky override (not reverted)");
}

// ============================================================================
// Gap #5 — terminate still runs the post-turn path + steering/follow-up (agent-loop.ts:210,218-262).
// ============================================================================

/// Forces `terminate = true` on every tool result (so the tool batch terminates) and counts how many
/// times the post-turn `prepare_next_turn` hook fires.
struct TerminateAndCount {
    prepare_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for TerminateAndCount {
    async fn after_tool_call(
        &self,
        _ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        Ok(Some(AfterOverride { terminate: Some(true), ..Default::default() }))
    }
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<Option<TurnUpdate>, HookError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

#[tokio::test]
async fn gap5_terminate_still_runs_post_turn_hooks_and_drains_follow_up() {
    // Turn 1 calls a tool (which terminates); turn 2 (only reached if the loop did NOT short-circuit
    // on terminate) is a plain stop after the queued follow-up is injected.
    let hook = Arc::new(TerminateAndCount { prepare_calls: AtomicUsize::new(0) });
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("after follow-up")], StopReason::Stop),
    ]).1;
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::named("echo")])
        .hooks(hook.clone())
        .build();

    // Enqueue a follow-up up front: it must be drained AFTER the terminating turn (Pi: terminate ends
    // only tool-driven continuation; queued follow-up still flows).
    agent.follow_up(AgentMessage::user_text("keep going"));

    let new = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // (a) The post-turn hook fired on the terminating turn — Pi runs prepareNextTurn after turn_end
    // even when terminate is set. The old short-circuit skipped it entirely.
    assert!(
        hook.prepare_calls.load(Ordering::SeqCst) >= 1,
        "prepare_next_turn must run on a terminating turn"
    );
    // (b) The queued follow-up still drove a second turn — terminate did NOT drop it.
    let asst_count = new.iter().filter(|m| m.is_assistant()).count();
    assert_eq!(asst_count, 2, "follow-up produced a second assistant turn despite terminate");
    let follow_visible = new.iter().any(|m| matches!(
        m,
        AgentMessage::User { content, .. } if matches!(content.first(), Some(Content::Text { text, .. }) if text == "keep going")
    ));
    assert!(follow_visible, "the follow-up message was injected, not dropped");
}

// ============================================================================
// Gap #6 — uncontained run failure reports the REAL panic message (agent.ts:496-511).
// ============================================================================

struct ExplodingHook;

#[async_trait::async_trait]
impl Hooks for ExplodingHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<Option<TurnUpdate>, HookError> {
        panic!("kaboom: hook detonated");
    }
}

#[tokio::test]
async fn gap6_run_failure_surfaces_real_panic_message_and_error_stop_reason() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)]).1;
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(ExplodingHook)).build();

    let new = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let failure = new
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Assistant(a) => Some(a.clone()),
            _ => None,
        })
        .expect("a synthetic failure assistant message");
    // Not aborted (no cancellation) => error.
    assert_eq!(failure.stop_reason, StopReason::Error, "uncancelled failure => error");
    // The real panic message is recovered (downcast), not a generic "run task failed".
    assert_eq!(
        failure.error_message.as_deref(),
        Some("kaboom: hook detonated"),
        "the real panic message must be surfaced (Pi error.message)"
    );
}

// ============================================================================
// AGENT-007 — a post-turn hook that RETURNS `Err` (Pi: throws) is a run FAILURE, not a quiet stop.
//
// Pi awaits `prepareNextTurn` (agent-loop.ts:231) and `shouldStopAfterTurn` (agent-loop.ts:246-252)
// bare — no try/catch — so a rejection escapes `runLoop` into `runWithLifecycle`'s catch
// (agent.ts:489-490) and reaches `handleRunFailure` (agent.ts:496-511), which emits a synthetic
// errored assistant message through `message_start` → `message_end` → `turn_end` (empty
// `toolResults`) → `agent_end` carrying `[failureMessage]`.
// ============================================================================

/// `prepare_next_turn` fails (Pi: throws) after the first turn completes.
struct FailingPrepareHook;

#[async_trait::async_trait]
impl Hooks for FailingPrepareHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<Option<TurnUpdate>, HookError> {
        Err(HookError::new("prepare exploded"))
    }
}

/// `should_stop_after_turn` fails (Pi: throws) after the first turn completes.
struct FailingStopHook;

#[async_trait::async_trait]
impl Hooks for FailingStopHook {
    async fn should_stop_after_turn(&self, _ctx: PostTurn<'_>, _cancel: CancelToken) -> Result<bool, HookError> {
        Err(HookError::new("stop check exploded"))
    }
}

#[tokio::test]
async fn agent007_failing_prepare_next_turn_emits_pis_full_failure_quartet() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)]).1;
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(FailingPrepareHook)).build();
    agent.subscribe(rec.clone());

    let new = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // The tail is Pi's `handleRunFailure` quartet, NOT a bare `agent_end` (agent.ts:508-511).
    let names = rec.names();
    let tail: Vec<&str> = names.iter().rev().take(4).rev().map(String::as_str).collect();
    assert_eq!(
        tail,
        vec!["message_start", "message_end", "turn_end", "agent_end"],
        "a failing prepare_next_turn must close with Pi's failure quartet, got {names:?}"
    );

    let events = rec.snapshot();
    // The synthetic failure message: `stopReason: "error"` (uncancelled) + the real error text
    // (agent.ts:504-505), one empty text block (agent.ts:499).
    let failure = last_assistant(&events);
    assert_eq!(failure.stop_reason, StopReason::Error);
    assert_eq!(failure.error_message.as_deref(), Some("prepare exploded"));
    assert_eq!(failure.content.len(), 1, "one EMPTY text block, not empty content");
    assert!(matches!(&failure.content[0], Content::Text { text, .. } if text.is_empty()));

    // `turn_end` carries the failure message and NO tool results (agent.ts:510).
    let te = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { message, tool_results } => {
                Some((message.clone(), tool_results.clone()))
            }
            _ => None,
        })
        .expect("a turn_end");
    assert!(te.1.is_empty(), "the failure turn_end carries no tool results");
    assert!(matches!(&te.0, AgentMessage::Assistant(a) if a.stop_reason == StopReason::Error));

    // `agent_end` carries EXACTLY `[failureMessage]` (agent.ts:511) — not the run's accumulator.
    let end = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::AgentEnd { messages } => Some(messages.clone()),
            _ => None,
        })
        .expect("an agent_end");
    assert_eq!(end.len(), 1, "agent_end carries only the failure message, got {end:?}");
    assert!(matches!(&end[0], AgentMessage::Assistant(a) if a.error_message.as_deref() == Some("prepare exploded")));

    // The run's returned messages match `agent_end`, exactly as the catch_unwind twin settles.
    assert_eq!(new.len(), 1);
    assert!(matches!(&new[0], AgentMessage::Assistant(a) if a.stop_reason == StopReason::Error));

    // The failure is visible on the agent's observable state, as it is for the panic path.
    assert_eq!(agent.snapshot().await.error_message.as_deref(), Some("prepare exploded"));
}

#[tokio::test]
async fn agent007_failing_should_stop_after_turn_emits_pis_full_failure_quartet() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)]).1;
    let rec = Arc::new(EventRecorder::default());
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(FailingStopHook)).build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let names = rec.names();
    let tail: Vec<&str> = names.iter().rev().take(4).rev().map(String::as_str).collect();
    assert_eq!(
        tail,
        vec!["message_start", "message_end", "turn_end", "agent_end"],
        "a failing should_stop_after_turn must close with Pi's failure quartet, got {names:?}"
    );

    let failure = last_assistant(&rec.snapshot());
    assert_eq!(failure.stop_reason, StopReason::Error);
    assert_eq!(failure.error_message.as_deref(), Some("stop check exploded"));
}

// ============================================================================
// Gap #3 — the low-level free-function loop primitives (agent-loop.ts:25,31-143).
// ============================================================================

#[tokio::test]
async fn gap3_run_agent_loop_pushes_ordered_events_and_returns_new_messages() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("hello")], StopReason::Stop)]).1;
    let sink = Arc::new(RecSink::default());
    let cancel = RunCancel::new();
    let ctx = AgentContext { system_prompt: "sys".into(), messages: Vec::new(), tools: Vec::new() };
    let config = AgentLoopConfig::new(model_ref());

    let new = run_agent_loop(
        vec![AgentMessage::user_text("hi")],
        ctx,
        config,
        sink.clone() as Arc<dyn AgentEventSink>,
        cancel,
        sf,
    )
    .await;

    // New messages = the prompt + the assistant response (Pi runAgentLoop returns newMessages).
    assert_eq!(new.len(), 2, "prompt + assistant");
    assert!(matches!(new[0], AgentMessage::User { .. }));
    assert!(new[1].is_assistant());

    // The sink saw the full ordered open/close sequence.
    let n = sink.names();
    assert_eq!(n.first().map(String::as_str), Some("agent_start"));
    assert_eq!(n.last().map(String::as_str), Some("agent_end"));
    assert_eq!(sink.turn_starts(), 1, "one turn");
    // message_start/end for the prompt user message precede the assistant message_start.
    let first_user = n.iter().position(|s| s == "message_start").unwrap();
    let turn_start = n.iter().position(|s| s == "turn_start").unwrap();
    assert!(turn_start < first_user, "turn_start precedes the prompt message_start");
}

#[tokio::test]
async fn gap3_agent_loop_pull_stream_finalizes_to_new_messages() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("yo")], StopReason::Stop)]).1;
    let cancel = RunCancel::new();
    let ctx = AgentContext::default();
    let config = AgentLoopConfig::new(model_ref());

    let mut stream = agent_loop(vec![AgentMessage::user_text("hi")], ctx, config, cancel, sf);

    // Pull events; the terminal agent_end carries the run's new messages.
    let mut saw_agent_start = false;
    let mut final_messages: Vec<AgentMessage> = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev {
            AgentEvent::AgentStart => saw_agent_start = true,
            AgentEvent::AgentEnd { messages } => final_messages = messages,
            _ => {}
        }
    }
    assert!(saw_agent_start, "stream yields agent_start");
    assert_eq!(final_messages.len(), 2, "agent_end carries prompt + assistant");
    assert!(final_messages[1].is_assistant());
}

#[tokio::test]
async fn gap3_run_agent_loop_continue_rejects_empty_and_trailing_assistant() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]).1;
    let sink = Arc::new(RecSink::default()) as Arc<dyn AgentEventSink>;

    // Empty transcript => NoMessages.
    let empty = run_agent_loop_continue(
        AgentContext::default(),
        AgentLoopConfig::new(model_ref()),
        sink.clone(),
        RunCancel::new(),
        sf.clone(),
    )
    .await;
    // AGENT-034 — the LOW-LEVEL surface's string, distinct from `Agent::continue_run`'s
    // (`agent-loop.ts:128` in `runAgentLoopContinue`, identical at both tags).
    assert!(matches!(empty, Err(crate::AgentError::NoMessages(crate::ContinueSurface::Loop))));
    assert_eq!(
        empty.err().map(|e| e.to_string()).unwrap_or_default(),
        "Cannot continue: no messages in context"
    );

    // Trailing assistant => ContinueFromAssistant.
    let ctx = AgentContext {
        system_prompt: String::new(),
        messages: vec![AgentMessage::Assistant(Arc::new(faux_assistant_message(
            vec![faux_text("prev")],
            StopReason::Stop,
        )))],
        tools: Vec::new(),
    };
    let trailing = run_agent_loop_continue(
        ctx,
        AgentLoopConfig::new(model_ref()),
        sink,
        RunCancel::new(),
        sf,
    )
    .await;
    assert!(matches!(trailing, Err(crate::AgentError::ContinueFromAssistant)));
    assert_eq!(
        trailing.err().map(|e| e.to_string()).unwrap_or_default(),
        "Cannot continue from message role: assistant"
    );
}

#[tokio::test]
async fn gap3_run_agent_loop_continue_resumes_from_user_tail() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("resumed")], StopReason::Stop)]).1;
    let sink = Arc::new(RecSink::default());
    // Transcript ends with a user message => a valid continuation point.
    let ctx = AgentContext {
        system_prompt: String::new(),
        messages: vec![AgentMessage::user_text("earlier question")],
        tools: Vec::new(),
    };

    let new = run_agent_loop_continue(
        ctx,
        AgentLoopConfig::new(model_ref()),
        sink.clone() as Arc<dyn AgentEventSink>,
        RunCancel::new(),
        sf,
    )
    .await
    .expect("continue resumes from a user tail");

    // Continuation does NOT re-add pre-existing context; only the new assistant message is returned.
    assert_eq!(new.len(), 1, "continuation returns only the new assistant message");
    assert!(new[0].is_assistant());
    assert_eq!(sink.names().last().map(String::as_str), Some("agent_end"));
}

#[tokio::test]
async fn gap3_agent_loop_continue_pull_stream_validates_up_front() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]).1;
    // A trailing assistant message must be rejected synchronously, before spawning.
    let ctx = AgentContext {
        system_prompt: String::new(),
        messages: vec![AgentMessage::Assistant(Arc::new(faux_assistant_message(
            vec![faux_text("prev")],
            StopReason::Stop,
        )))],
        tools: Vec::new(),
    };
    let res = agent_loop_continue(ctx, AgentLoopConfig::new(model_ref()), RunCancel::new(), sf);
    assert!(matches!(res, Err(crate::AgentError::ContinueFromAssistant)));
}

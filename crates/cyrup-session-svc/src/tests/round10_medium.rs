//! Round-10 MEDIUM parity tests.
//!
//!   * SEAM-121 — `steer`/`follow_up` run the `input` extension handlers, in pi's
//!     `_queueUserInput` order (agent-session.ts:1823-1848 @v0.87.1).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{InputSource, PromptAccepted, SessionBuilder, SessionConfig, UserInput};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::ExtError;
use cyrup_ext::{
    EventKind, EventPatch, HandledValue, HookOutcome, HostCtx, HostEvent, InitApi,
    InputEventSource, InputStreamingBehavior, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use tempfile::TempDir;
use tokio_stream::StreamExt;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    faux
}

// ============================================== SEAM-121 queued submissions run `input` handlers ==

type Seen = Arc<Mutex<Vec<(InputEventSource, Option<InputStreamingBehavior>, String)>>>;

/// An `input` handler that records what it was delivered and rewrites the text to `X`
/// (pi `action:"transform"`, runner.ts:1116-1119).
struct TransformInput(Seen);
#[async_trait::async_trait]
impl NativeExtension for TransformInput {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("seam121-transform")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::Input {
            text,
            source,
            streaming_behavior,
            ..
        } = ev
        {
            self.0
                .lock()
                .unwrap()
                .push((*source, *streaming_behavior, text.clone()));
            return HookOutcome::Mutate(EventPatch::Input {
                text: "X".to_string(),
                images: None,
            });
        }
        HookOutcome::Noop
    }
}

/// An `input` handler that fully services every submission (pi `action:"handled"`).
struct HandleInput;
#[async_trait::async_trait]
impl NativeExtension for HandleInput {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("seam121-handled")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { .. } => HookOutcome::Handled(HandledValue(serde_json::json!({
                "action": "handled"
            }))),
            _ => HookOutcome::Noop,
        }
    }
}

/// SEAM-121 — pi's `steer` is one line onto `_queueUserInput` (agent-session.ts:1858-1862
/// @v0.87.1), which runs `_runInputHandlers` before it expands or queues anything (`:1833`). cyrup's
/// `steer`/`follow_up` skipped the `input` event entirely, so a queued submission — every RPC
/// `steer`/`followUp` frame among them — reached the transcript with no handler ever seeing it.
#[tokio::test]
async fn seam121_steer_runs_the_input_handlers() {
    let fx = fixture();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(TransformInput(seen.clone())))
        .build()
        .await
        .unwrap();

    let accepted = session
        .steer(UserInput::text("a", InputSource::Rpc))
        .await
        .unwrap();
    assert_eq!(
        accepted,
        PromptAccepted::Queued(crate::StreamingBehavior::Steer)
    );

    let observed = seen.lock().unwrap().clone();
    assert_eq!(observed.len(), 1, "the input handler fired exactly once");
    assert_eq!(
        observed[0].0,
        InputEventSource::Rpc,
        "the handler observes the caller's source (pi `options?.source ?? \"interactive\"`)"
    );
    assert_eq!(
        observed[0].1, None,
        "no run is live, so pi passes `undefined` for streamingBehavior (agent-session.ts:1836)"
    );
    assert_eq!(
        observed[0].2, "a",
        "the handler sees the raw, unexpanded text"
    );
    assert_eq!(
        session.steering_messages(),
        vec!["X".to_string()],
        "the queue carries the handler's TRANSFORMED text, not the original"
    );

    // …and `follow_up` takes the same path (pi `:1871-1875`).
    let _ = session
        .follow_up(UserInput::text("b", InputSource::Rpc))
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert_eq!(session.follow_up_messages(), vec!["X".to_string()]);
}

/// SEAM-121 — `if (!processedInput) return;` (agent-session.ts:1839 @v0.87.1): a `handled` outcome
/// queues NOTHING and emits no `queue_update`.
#[tokio::test]
async fn seam121_steer_handled_queues_nothing() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(HandleInput))
        .build()
        .await
        .unwrap();
    let mut events = session.subscribe();

    let accepted = session
        .steer(UserInput::text("a", InputSource::Rpc))
        .await
        .unwrap();
    assert_eq!(accepted, PromptAccepted::Handled);
    assert!(
        session.steering_messages().is_empty(),
        "a handled submission is not queued"
    );
    assert_eq!(
        session.pending_message_count(),
        0,
        "nothing is pending after a handled submission"
    );
    // No `queue_update` was emitted: the stream has nothing waiting.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.next())
            .await
            .is_err(),
        "a handled submission emits no queue_update"
    );
}

/// Presence before absence for the `_queueSteer` split: `send_custom_message` is NOT user input
/// (pi calls `this.agent.steer(appMessage)` directly, agent-session.ts:1949-1955 @v0.87.1), so the
/// `input` handlers must never see it and must never rewrite it. This guards the split — it fails if
/// the custom-message path is ever routed through the public `steer`.
#[tokio::test]
async fn seam121_custom_message_does_not_run_input_handlers() {
    let fx = fixture();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(TransformInput(seen.clone())))
        .build()
        .await
        .unwrap();

    session
        .send_custom_message(
            "note",
            serde_json::json!({"body": "verbatim"}),
            true,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        seen.lock().unwrap().is_empty(),
        "a custom message is not user input and must not dispatch `input`"
    );
}

// ==================================== SEAM-127 `triggerTurn: false` while streaming is deferred ==

/// The marker a test looks for in the provider requests. If it is in one, the custom message was
/// steered onto the loop and the model was asked to answer it.
const DEFERRED_MARKER: &str = "DEFERRED_NOTE_MARKER";

type Requests = Arc<Mutex<Vec<String>>>;
type SessionSlot = Arc<std::sync::OnceLock<std::sync::Weak<crate::AgentSession>>>;

/// A tool that sends a custom message MID-RUN, with the delivery options under test.
struct Notifier {
    slot: SessionSlot,
    deliver_as: Option<crate::DeliverAs>,
    trigger_turn: Option<bool>,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl cyrup_core::Tool for Notifier {
    fn name(&self) -> &str {
        "notify"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _params: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        let session = self
            .slot
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| cyrup_core::ToolError::new("no session"))?;
        session
            .send_custom_message(
                "note",
                serde_json::json!({"body": DEFERRED_MARKER}),
                true,
                None,
                self.deliver_as,
                self.trigger_turn,
            )
            .await
            .map_err(|e| cyrup_core::ToolError::new(e.to_string()))?;
        Ok(cyrup_core::ToolResult {
            content: vec![cyrup_core::Content::text("notified")],
            ..Default::default()
        })
    }
}

/// Turn 1 calls `notify`, every later turn stops. Each step records the request's message list so a
/// test can see whether the custom message was handed to the model.
fn faux_tool_then_stop(requests: &Requests) -> Arc<FauxProvider> {
    use cyrup_provider::faux::{FauxResponseStep, faux_tool_call};
    let mk = |requests: &Requests, call: bool| {
        let cap = requests.clone();
        FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
            cap.lock().unwrap().push(format!("{:?}", ctx.messages));
            if call {
                faux_assistant_message(
                    vec![faux_tool_call("notify".to_string(), serde_json::json!({}))],
                    StopReason::ToolUse,
                )
            } else {
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            }
        })
    };
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        mk(requests, true),
        mk(requests, false),
        mk(requests, false),
    ]);
    faux
}

async fn session_with_notifier(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    deliver_as: Option<crate::DeliverAs>,
    trigger_turn: Option<bool>,
) -> Arc<crate::AgentSession> {
    let slot: SessionSlot = Arc::new(std::sync::OnceLock::new());
    let mut cfg = base_config(fx);
    cfg.no_extensions = true;
    cfg.custom_tools = vec![Arc::new(Notifier {
        slot: slot.clone(),
        deliver_as,
        trigger_turn,
        params: serde_json::json!({ "type": "object", "properties": {} }),
    }) as Arc<dyn cyrup_core::Tool>];
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::downgrade(&session));
    // Make `notify` the active set, exactly as the mid_run_tool_anchoring harness does.
    session
        .set_active_tools_by_name(&["notify".to_string()])
        .await;
    session
}

/// Whether the custom entry reached the durable session tree.
async fn persisted_note(session: &Arc<crate::AgentSession>) -> bool {
    use cyrup_session::agent_message::AgentMessage as Raw;
    session
        .raw_context_messages()
        .await
        .into_iter()
        .any(|m| matches!(m, Raw::Custom(c) if c.custom_type == "note"))
}

/// SEAM-127 — pi's branch 4 (agent-session.ts:1961-1966 @v0.87.1): with a run live and
/// `triggerTurn: false`, the custom message is DEFERRED to the end of the turn, never steered onto
/// the loop. cyrup collapsed an explicit `false` onto an absent key, so the message took the
/// streaming steer arm and the model was asked to answer it — the one outcome `triggerTurn: false`
/// exists to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam127_trigger_turn_false_during_a_run_starts_no_turn() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_tool_then_stop(&requests);
    let session = session_with_notifier(&fx, faux, None, Some(false)).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let seen = requests.lock().unwrap().clone();
    assert!(
        seen.len() >= 2,
        "the run drove the tool turn: {}",
        seen.len()
    );
    assert!(
        seen.iter().all(|r| !r.contains(DEFERRED_MARKER)),
        "a `triggerTurn: false` message must never be handed to the model: {seen:?}"
    );
    // …and it is not LOST: the deferred message is appended at the turn boundary.
    assert!(
        persisted_note(&session).await,
        "the deferred custom message must still reach the session tree"
    );
}

/// SEAM-127 — pi's branch 2 is `this.isStreaming && options?.triggerTurn !== false`
/// (agent-session.ts:1949 @v0.87.1). An ABSENT `triggerTurn` is not `false`, so the message IS
/// steered onto the live loop. This pins the `!== false` semantics: it fails if the deferral guard is
/// written as `trigger_turn == Some(true)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam127_absent_trigger_turn_still_steers() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_tool_then_stop(&requests);
    let session = session_with_notifier(&fx, faux, None, None).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let seen = requests.lock().unwrap().clone();
    assert!(
        seen.iter().any(|r| r.contains(DEFERRED_MARKER)),
        "with `triggerTurn` absent the message is steered onto the loop: {seen:?}"
    );
}

/// SEAM-127 — pi's branch 2 requires `triggerTurn !== false`, so `deliverAs: "followUp"` with
/// `triggerTurn: false` falls THROUGH to branch 4 and is deferred, not queued. The deferral arm
/// therefore has to sit after the `nextTurn` arm but be reachable for any other `deliverAs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam127_deliver_as_follow_up_with_trigger_turn_false_is_deferred_not_queued() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_tool_then_stop(&requests);
    let session =
        session_with_notifier(&fx, faux, Some(crate::DeliverAs::FollowUp), Some(false)).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let seen = requests.lock().unwrap().clone();
    assert!(
        seen.iter().all(|r| !r.contains(DEFERRED_MARKER)),
        "`deliverAs: followUp` + `triggerTurn: false` is deferred, not queued: {seen:?}"
    );
    assert!(
        session.follow_up_messages().is_empty(),
        "nothing was queued onto the follow-up mirror"
    );
    assert!(
        persisted_note(&session).await,
        "the deferred message lands via the flush"
    );
}

/// SEAM-127 — pi emits NOTHING when it defers ("message events must not describe messages the
/// session tree does not contain", agent-session.ts:1963-1965 @v0.87.1), and the append happens at
/// the `turn_end` boundary, i.e. after the turn's tool results are in (`:965-973`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam127_deferred_message_is_emitted_only_after_the_turns_tool_results() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_tool_then_stop(&requests);
    let session = session_with_notifier(&fx, faux, None, Some(false)).await;

    let mut events = session.subscribe();
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    // Collect the finalized-message stream in order.
    let mut order: Vec<String> = Vec::new();
    while let Ok(Some(ev)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), events.next()).await
    {
        if let crate::AgentSessionEvent::MessageEnd { message } = ev {
            match &message {
                cyrup_agent::AgentMessage::Custom { kind, .. } => {
                    order.push(format!("custom:{kind}"))
                }
                cyrup_agent::AgentMessage::ToolResult { .. } => {
                    order.push("tool_result".to_string())
                }
                _ => {}
            }
        }
    }
    let note_at = order.iter().position(|e| e == "custom:note");
    let tool_at = order.iter().position(|e| e == "tool_result");
    let (note_at, tool_at) = match (note_at, tool_at) {
        (Some(n), Some(t)) => (n, t),
        other => panic!("expected both a tool result and the deferred note: {other:?} / {order:?}"),
    };
    assert!(
        note_at > tool_at,
        "the deferred note is appended AFTER the turn's tool result: {order:?}"
    );
}

/// SEAM-127 — WHERE the deferred message is displayed. Pi flushes in `_handleAgentEvent`'s
/// `turn_end` arm (`agent-session.ts:965-973` @v0.87.1), i.e. INSIDE the run, at the boundary
/// immediately after the turn that deferred it: "A turn ends after its assistant message and every
/// tool result has been appended, so this is the first point in the run where a context-only custom
/// message can be inserted without landing between a tool call and its result." The run's `finally`
/// (`:1486-1487`) is only the backstop for a run that never reaches a `turn_end`.
///
/// So the note's `message_end` must fall between the FIRST `turn_end` and the SECOND turn's
/// assistant message — and therefore well before `agent_end`/`agent_settled`. A flush that only
/// lands in cyrup's `settle_run` puts it after `agent_end` instead: same message, later display.
/// This test is what separates the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam127_deferred_message_is_displayed_at_the_turn_boundary_not_at_settle() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_tool_then_stop(&requests);
    let session = session_with_notifier(&fx, faux, None, Some(false)).await;

    let mut events = session.subscribe();
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let mut order: Vec<String> = Vec::new();
    while let Ok(Some(ev)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), events.next()).await
    {
        match &ev {
            crate::AgentSessionEvent::MessageEnd { message } => match message {
                cyrup_agent::AgentMessage::Custom { kind, .. } => {
                    order.push(format!("custom:{kind}"));
                }
                cyrup_agent::AgentMessage::Assistant(_) => order.push("assistant".to_string()),
                cyrup_agent::AgentMessage::ToolResult { .. } => {
                    order.push("tool_result".to_string());
                }
                _ => {}
            },
            crate::AgentSessionEvent::TurnEnd { .. } => order.push("turn_end".to_string()),
            crate::AgentSessionEvent::AgentEnd { .. } => order.push("agent_end".to_string()),
            crate::AgentSessionEvent::AgentSettled => order.push("agent_settled".to_string()),
            _ => {}
        }
    }

    let at = |needle: &str| order.iter().position(|e| e == needle);
    let Some(note_at) = at("custom:note") else {
        panic!("the deferred note was never displayed: {order:?}");
    };
    let Some(first_turn_end) = at("turn_end") else {
        panic!("no turn boundary was observed: {order:?}");
    };
    let Some(agent_end) = at("agent_end") else {
        panic!("the run never ended: {order:?}");
    };
    // The SECOND turn's assistant message — the one the loop streams after the tool turn. The note
    // has to be displayed before it, which is only possible from the `turn_end` flush.
    let Some(second_assistant) = order
        .iter()
        .skip(first_turn_end)
        .position(|e| e == "assistant")
        .map(|offset| first_turn_end + offset)
    else {
        panic!("the run did not drive a second turn: {order:?}");
    };

    assert!(
        note_at > first_turn_end,
        "the note is displayed AFTER the turn that deferred it has ended, \
         never between a tool call and its result: {order:?}"
    );
    assert!(
        note_at < second_assistant,
        "the note is displayed at the turn boundary, before the next turn's assistant \
         message — not held back to the run's settle: {order:?}"
    );
    assert!(
        note_at < agent_end,
        "the note is displayed INSIDE the run, before `agent_end` — pi flushes at `turn_end` \
         (agent-session.ts:965-973), not in the run's `finally`: {order:?}"
    );
    if let Some(settled) = at("agent_settled") {
        assert!(
            note_at < settled,
            "the note is displayed before the run settles: {order:?}"
        );
    }
}

/// SEAM-127 + ICOM-068 composed — the deferral arm and the append it flushes through are pi's
/// branches 4 and 5 of one `if`/`else` chain (`agent-session.ts:1961-1969` @v0.87.1), and BOTH
/// halves of `_appendCustomMessage` (`:1972-1982`) apply to the flush: the tree append and
/// `_refreshFinalizedContext()` (`:1979`). So a `triggerTurn: false` message deferred during a run
/// is never handed to that run, and IS in the agent transcript — and therefore in the next prompt's
/// request — once the flush has run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam127_deferred_message_reaches_the_model_on_the_next_prompt() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_tool_then_stop(&requests);
    let session = session_with_notifier(&fx, faux, None, Some(false)).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let during_run = {
        let seen = requests.lock().unwrap().clone();
        assert!(
            seen.iter().all(|r| !r.contains(DEFERRED_MARKER)),
            "the deferred message is never handed to the run that deferred it: {seen:?}"
        );
        seen.len()
    };
    assert!(
        session
            .agent_messages()
            .await
            .iter()
            .any(|m| matches!(m, cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "note")),
        "the flush appends to the AGENT transcript, not only the session tree (ICOM-068)"
    );

    let _ = session.prompt("again").await.unwrap();
    session.wait_for_idle().await;

    let seen = requests.lock().unwrap().clone();
    assert!(
        seen.len() > during_run,
        "the second prompt reached the provider: {seen:?}"
    );
    assert!(
        seen.iter()
            .skip(during_run)
            .any(|r| r.contains(DEFERRED_MARKER)),
        "the deferred message rides the NEXT prompt's request: {seen:?}"
    );
}

// ============================== SEAM-126 threshold compaction at the per-turn boundary ==========

/// A tool whose result is large enough to matter to the context estimate.
struct Bulky(serde_json::Value);
#[async_trait::async_trait]
impl cyrup_core::Tool for Bulky {
    fn name(&self) -> &str {
        "bulky"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.0
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _params: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult {
            content: vec![cyrup_core::Content::text("payload ".repeat(400))],
            ..Default::default()
        })
    }
}

/// Turn 1 calls `bulky`; every later turn stops.
fn faux_bulky_loop() -> Arc<FauxProvider> {
    use cyrup_provider::faux::{FauxResponseStep, faux_tool_call};
    let step = |call: bool| {
        FauxResponseStep::factory(move |_ctx, _opts, _state, _model| {
            if call {
                faux_assistant_message(
                    vec![faux_tool_call("bulky".to_string(), serde_json::json!({}))],
                    StopReason::ToolUse,
                )
            } else {
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            }
        })
    };
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![step(true), step(false), step(false), step(false)]);
    faux
}

/// `reserveTokens` large enough that any real context is already over the threshold.
fn over_threshold_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 127999}),
    )
    .unwrap();
    cli
}

/// `reserveTokens: 0` — nothing this small can cross the threshold.
fn under_threshold_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

async fn run_bulky_loop(settings: cyrup_config::Settings) -> Vec<&'static str> {
    let fx = fixture();
    let mut cfg = base_config(&fx);
    cfg.no_extensions = true;
    cfg.custom_tools = vec![Arc::new(Bulky(
        serde_json::json!({ "type": "object", "properties": {} }),
    )) as Arc<dyn cyrup_core::Tool>];
    let session = SessionBuilder::new(faux_bulky_loop() as Arc<dyn Provider>, cfg)
        .cli_settings(settings)
        .build()
        .await
        .unwrap()
        .into_shared();
    session
        .set_active_tools_by_name(&["bulky".to_string()])
        .await;

    let mut stream = session.subscribe();
    let running = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            let _ = session.prompt("go").await;
            session.wait_for_idle().await;
        }
    });
    let mut kinds: Vec<&'static str> = Vec::new();
    while let Ok(Some(ev)) =
        tokio::time::timeout(std::time::Duration::from_secs(20), stream.next()).await
    {
        let k = ev.kind();
        kinds.push(k);
        if k == "agent_settled" {
            break;
        }
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(20), running).await;
    kinds
}

/// SEAM-126 — pi checks the compaction threshold at EVERY turn boundary, as the first statement of
/// its `prepareNextTurnWithContext` override (`_compactBeforeNextAssistantResponse`,
/// agent-session.ts:587-605 called from `:693-694` @v0.87.1, added in v0.84.4 / #8782). cyrup's only
/// threshold check was `check_compaction`, reached from `handle_post_agent_run` AFTER `agent_end` —
/// so a run whose own tool results grew the context past the threshold kept driving turn after turn
/// against a context nothing was shrinking, and the first compaction came only once the whole run
/// had ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam126_threshold_compaction_runs_before_the_next_provider_request() {
    let kinds = run_bulky_loop(over_threshold_settings()).await;
    let agent_end_at = kinds
        .iter()
        .position(|k| *k == "agent_end")
        .unwrap_or(kinds.len());
    assert!(
        kinds.contains(&"turn_end"),
        "the tool loop drove at least one turn: {kinds:?}"
    );
    assert!(
        kinds
            .iter()
            .take(agent_end_at)
            .any(|k| *k == "compaction_start"),
        "the threshold compaction must run INSIDE the run, at a turn boundary, not only after \
         `agent_end`: {kinds:?}"
    );
}

/// The negative half: the new arm must not compact unconditionally. Under a reserve of 0 this same
/// loop is nowhere near its window, so no compaction may fire at all — anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seam126_no_compaction_when_under_threshold() {
    let kinds = run_bulky_loop(under_threshold_settings()).await;
    assert!(
        kinds.contains(&"turn_end"),
        "the tool loop drove at least one turn: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"compaction_start"),
        "a context well under the threshold must not be compacted: {kinds:?}"
    );
}

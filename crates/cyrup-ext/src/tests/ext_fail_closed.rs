//! EXT-001 — a FAULTING `tool_call` handler must BLOCK the tool call (fail CLOSED), end to end.
//!
//! `tool_call` is cyrup's permission seam (R-08-010): `cyrup-permission-system` subscribes exactly
//! `EventKind::ToolCall` and consumes the `before_tool_call` block. Before this fix a handler that
//! trapped, panicked, or merely blew the ~5s invocation budget was reported-and-SKIPPED, so
//! `dispatch_block_mutate` returned `Reduced::Pass` and `ExtHooks::before_tool_call` returned
//! `BeforeOutcome::Proceed` — the ungated tool ran. That is fail-OPEN on a security boundary.
//!
//! pi fails closed. `pi/packages/coding-agent/src/core/extensions/runner.ts:932-953` (`emitToolCall`)
//! is the ONLY emitter with no per-handler try/catch; `agent-session.ts:475-487` re-throws the fault
//! as `Extension failed, blocking execution: …`; `agent-loop.ts:616-662` catches that and returns
//! `{kind:"immediate", result: createErrorToolResult(...), isError: true}` — the tool is never
//! executed.
//!
//! These tests prove the OBSERVABLE consequence, not the returned status: every tool here flips an
//! `AtomicBool` as the first statement of `execute`, a real `cyrup_agent::Agent` is driven by the
//! scripted faux provider with `ExtensionHost::hooks()` installed, and the assertion is that the
//! flag is still `false`. The final test pins the other direction — a handler that DECLINED to block
//! (returned `Noop`) is not a fault and the tool still runs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{
    Dispatcher, EventKind, ExtError, ExtHooks, ExtKind, ExtMode, Extension, ExtensionError,
    ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi, NativeExtension,
    NativeHandle, Reduced, Subscriptions,
};
use cyrup_agent::{Agent, AgentEvent, AgentMessage, EventSubscriber, ProviderStreamFn, StreamFn};
use cyrup_core::{
    CancelToken, Content, ExtensionId, ModelRef, StopReason, TerminateHint, Tool, ToolCallId,
    ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text, faux_tool_call};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Harness: a tool that records whether it ran, plus a scripted one-tool-call agent turn.
// ---------------------------------------------------------------------------

/// A tool whose ONLY job is to record that `execute` was entered. `ran` is the observable side
/// effect the fail-closed assertions read.
struct TripwireTool {
    params: Value,
    ran: Arc<AtomicBool>,
}

impl TripwireTool {
    fn new() -> (Arc<Self>, Arc<AtomicBool>) {
        let ran = Arc::new(AtomicBool::new(false));
        let params = json!({ "type": "object", "properties": {}, "additionalProperties": true });
        (
            Arc::new(Self {
                params,
                ran: ran.clone(),
            }),
            ran,
        )
    }
}

#[async_trait::async_trait]
impl Tool for TripwireTool {
    fn name(&self) -> &str {
        "danger"
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
        Ok(ToolResult {
            content: vec![Content::text("executed")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSubscriber for Recorder {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
        if let Ok(mut g) = self.events.lock() {
            g.push(event.clone());
        }
    }
}

impl Recorder {
    /// The first tool-result message the loop produced (blocked or executed).
    fn tool_result(&self) -> cyrup_agent::ToolResultMessage {
        let events = self.events.lock().unwrap().clone();
        events
            .iter()
            .find_map(|e| match e {
                AgentEvent::MessageStart {
                    message: AgentMessage::ToolResult(t),
                } => Some(t.clone()),
                _ => None,
            })
            .expect("a tool-result message")
    }
}

fn model_ref() -> ModelRef {
    ModelRef {
        provider: "faux".into(),
        api: Some("faux".into()),
        model: "faux-1".into(),
    }
}

/// Script one assistant turn that calls `danger`, then a plain text turn so the loop terminates.
fn one_tool_call_stream_fn() -> Arc<dyn StreamFn> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("danger", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    Arc::new(ProviderStreamFn::new(provider))
}

/// Drive one full agent run with `hooks` installed and the tripwire tool registered.
/// Returns `(tool_ran, recorder)`.
async fn run_with_hooks(hooks: Arc<dyn cyrup_agent::Hooks>) -> (bool, Arc<Recorder>) {
    let (tool, ran) = TripwireTool::new();
    let agent = Agent::builder(model_ref(), one_tool_call_stream_fn())
        .tools(vec![tool as Arc<dyn Tool>])
        .hooks(hooks)
        .build();
    let recorder = Arc::new(Recorder::default());
    agent.subscribe(recorder.clone());
    agent.prompt("go").await.unwrap();
    agent.wait_for_idle().await;
    (ran.load(Ordering::SeqCst), recorder)
}

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

fn assert_blocked(result: &cyrup_agent::ToolResultMessage) {
    assert!(
        result.is_error,
        "a fault-blocked call yields an isError tool result"
    );
    let text = match &result.content[0] {
        Content::Text { text, .. } => text.to_string(),
        other => panic!("expected text content, got {other:?}"),
    };
    assert!(
        text.contains("Extension failed, blocking execution"),
        "pi's blocking message reaches the model (agent-session.ts:475-487): {text}"
    );
}

// ---------------------------------------------------------------------------
// Fault 1 — a PANICKING handler (native panic containment ⇒ ExtError::Panicked).
// ---------------------------------------------------------------------------

struct PanickingGate;

#[async_trait::async_trait]
impl NativeExtension for PanickingGate {
    fn id(&self) -> ExtensionId {
        "panicking-gate".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        panic!("gate exploded before it could decide");
    }
}

#[tokio::test]
async fn ext001_panicking_tool_call_handler_blocks_the_tool() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PanickingGate)).await.unwrap();

    let (tool_ran, recorder) = run_with_hooks(host.hooks()).await;

    assert!(
        !tool_ran,
        "EXT-001: a panicking tool_call gate must NOT let the tool execute"
    );
    assert_blocked(&recorder.tool_result());
}

// ---------------------------------------------------------------------------
// Fault 2 — a handler that RETURNS an error (guest trap / OOM / serialization failure all reach
// the dispatcher as exactly this `Err` from `Extension::invoke_event`).
// ---------------------------------------------------------------------------

/// An `Extension` implemented directly so `invoke_event` can return `Err` — the shape a wasm trap,
/// an OOM, a cancelled/unloaded instance, and a (de)serialization failure all collapse to.
struct ErroringGate {
    id: ExtensionId,
    subs: Subscriptions,
}

#[async_trait::async_trait]
impl Extension for ErroringGate {
    fn id(&self) -> &ExtensionId {
        &self.id
    }
    fn kind(&self) -> ExtKind {
        ExtKind::Native
    }
    fn subscriptions(&self) -> Subscriptions {
        self.subs
    }
    async fn invoke_event(
        &self,
        _ev: &HostEvent,
        _cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        Err(ExtError::Trap("guest trapped".into()))
    }
}

#[tokio::test]
async fn ext001_erroring_tool_call_handler_blocks_the_tool() {
    let dispatcher = Arc::new(Dispatcher::new());
    dispatcher
        .add(Arc::new(ErroringGate {
            id: "erroring-gate".into(),
            subs: Subscriptions::empty().with(EventKind::ToolCall),
        }))
        .unwrap();

    let hooks: Arc<dyn cyrup_agent::Hooks> = Arc::new(ExtHooks::new(dispatcher));
    let (tool_ran, recorder) = run_with_hooks(hooks).await;

    assert!(
        !tool_ran,
        "EXT-001: a trapping tool_call gate must NOT let the tool execute"
    );
    assert_blocked(&recorder.tool_result());
}

// ---------------------------------------------------------------------------
// Fault 3 — INVOCATION-BUDGET exhaustion (the ~5s DEFAULT_INVOKE_BUDGET / wasm epoch deadline).
// A runaway gate that never reaches a decision must deny, not allow.
// ---------------------------------------------------------------------------

struct RunawayGate {
    wait: Duration,
}

#[async_trait::async_trait]
impl NativeExtension for RunawayGate {
    fn id(&self) -> ExtensionId {
        "runaway-gate".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        // No human-wait guard held ⇒ a cooperative runaway, not a sanctioned human wait (P-3).
        tokio::time::sleep(self.wait).await;
        HookOutcome::Block {
            reason: Some("never observed — budget fires first".into()),
            terminate: TerminateHint::Unspecified,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext001_budget_exhausted_tool_call_handler_blocks_the_tool() {
    // A short budget so the test does not wait out the 5s production default.
    let dispatcher = Arc::new(Dispatcher::with_budget(Duration::from_millis(80)));
    let ctx = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    dispatcher
        .add(Arc::new(NativeHandle::new(
            Arc::new(RunawayGate {
                wait: Duration::from_millis(600),
            }),
            Subscriptions::empty().with(EventKind::ToolCall),
            ctx,
        )))
        .unwrap();

    let hooks: Arc<dyn cyrup_agent::Hooks> = Arc::new(ExtHooks::new(dispatcher));
    let (tool_ran, recorder) = run_with_hooks(hooks).await;

    assert!(
        !tool_ran,
        "EXT-001: a budget-timed-out tool_call gate must NOT let the tool execute"
    );
    assert_blocked(&recorder.tool_result());
}

// ---------------------------------------------------------------------------
// The fault is still SURFACED: fail-closed does not replace the R-08-036 error-listener report.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ext001_fault_is_reported_and_blocked() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PanickingGate)).await.unwrap();

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    host.add_error_listener(Arc::new(move |e: &ExtensionError| {
        if let Ok(mut g) = sink.lock() {
            g.push((e.extension.to_string(), e.event.to_string()));
        }
    }));

    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "t".into(),
                name: "danger".into(),
                input: json!({}),
            },
            &CancelToken::new(),
        )
        .await;

    match reduced {
        Reduced::Blocked { reason, by, .. } => {
            assert_eq!(
                by.to_string(),
                "panicking-gate",
                "attributed to the faulting extension"
            );
            let reason = reason.expect("a block reason");
            assert!(
                reason.contains("Extension failed, blocking execution"),
                "pi's wording (agent-session.ts:481): {reason}"
            );
        }
        other => panic!("EXT-001: a faulting tool_call handler must Block, got {other:?}"),
    }

    let got = captured.lock().unwrap().clone();
    assert_eq!(
        got.len(),
        1,
        "the fault is still reported to error listeners (R-08-036)"
    );
    assert_eq!(got[0].0, "panicking-gate");
    assert_eq!(got[0].1, "tool_call");
}

// ---------------------------------------------------------------------------
// The OTHER direction: DECLINING to block is not a fault. A handler that returns `Noop` still lets
// the tool run — the fix must not turn every gate into a deny.
// ---------------------------------------------------------------------------

struct PermissiveGate;

#[async_trait::async_trait]
impl NativeExtension for PermissiveGate {
    fn id(&self) -> ExtensionId {
        "permissive-gate".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn ext001_declining_to_block_still_executes_the_tool() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PermissiveGate)).await.unwrap();

    let (tool_ran, recorder) = run_with_hooks(host.hooks()).await;

    assert!(
        tool_ran,
        "a gate that DECLINED to block is not a fault — the tool proceeds"
    );
    assert!(
        !recorder.tool_result().is_error,
        "and the result is not an error"
    );
}

// ---------------------------------------------------------------------------
// Fail-OPEN is preserved for the kinds pi genuinely catches (runner.ts per-handler try/catch on
// emitContext/emitToolResult/emitInput/emitUserBash/emitBeforeAgentStart/emitResourcesDiscover/emit).
// ---------------------------------------------------------------------------

#[test]
fn only_tool_call_fails_closed() {
    for v in 0..EventKind::COUNT {
        let kind = EventKind::from_u8(v).expect("every discriminant below COUNT parses");
        assert_eq!(
            kind.fails_closed(),
            kind == EventKind::ToolCall,
            "{} fail-closed policy must match pi's runner.ts try/catch coverage",
            kind.name()
        );
    }
}

#[tokio::test]
async fn a_faulting_context_handler_still_fails_open() {
    struct PanicOnContext;
    #[async_trait::async_trait]
    impl NativeExtension for PanicOnContext {
        fn id(&self) -> ExtensionId {
            "panic-on-context".into()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe(&[EventKind::Context]);
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            panic!("boom");
        }
    }

    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PanicOnContext)).await.unwrap();

    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::Context {
                messages: Vec::new(),
            },
            &CancelToken::new(),
        )
        .await;

    assert!(
        matches!(reduced, Reduced::Pass(_)),
        "pi's emitContext catches per handler (runner.ts:993-1000) — cyrup must too"
    );
}

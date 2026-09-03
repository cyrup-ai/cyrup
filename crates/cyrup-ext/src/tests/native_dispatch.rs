//! Native built-in dispatch/registration/seam/containment contracts (arch-08 §11). These drive the
//! FULL extension surface WITHOUT any wasm — native built-in extensions exercise every contract.
//! Maps to acceptance criteria A-08-1..5, R-08-034 (gated dispatch), and R-08-036 (containment).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::{
    AfterToolCall, AgentContextView, AgentEvent, AgentMessage, BeforeOutcome, BeforeToolCall,
};
use cyrup_core::{
    TerminateHint,CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult};
use crate::{
    CommandDescriptor, EventKind, ExtMode, ExtensionError, ExtensionHost, HookOutcome, HostConfig,
    HostCtx, HostEvent, InitApi, NativeExtension, Reduced,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: std::path::PathBuf::from(".") }
}

/// Build the triggering assistant message + standalone `ToolCall` (same id/name/arguments as the
/// hook under test) that the enriched `BeforeToolCall`/`AfterToolCall` contexts now carry. Returns
/// both so each is borrowed into the context and outlives that borrow.
fn tool_call_msg(
    tool_name: &str,
    tool_call_id: &ToolCallId,
    args: &Value,
) -> (cyrup_core::AssistantMessage, cyrup_core::ToolCall) {
    let arguments = match args {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    let tc = cyrup_core::ToolCall {
        id: tool_call_id.clone(),
        name: tool_name.to_string(),
        arguments: arguments.into(),
        thought_signature: None,
    };
    let msg = cyrup_core::AssistantMessage {
        content: vec![Content::ToolCall(tc.clone())],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: cyrup_core::StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    };
    (msg, tc)
}

/// An empty read-only context view (no system prompt / messages / tools) for hook-context fields.
fn empty_view() -> AgentContextView<'static> {
    AgentContextView { system_prompt: "", messages: &[], tools: &[] }
}

// ---------------------------------------------------------------------------
// A-08-1: every event fires with its payload + the notify contract holds.
// ---------------------------------------------------------------------------
struct ProbeExt {
    id: ExtensionId,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for ProbeExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::AgentStart, EventKind::ToolExecEnd]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let label = match ev {
            HostEvent::AgentStart => "agent_start".to_string(),
            HostEvent::ToolExecEnd { call_id, is_error, .. } => {
                format!("tool_exec_end:{call_id}:{is_error}")
            }
            other => format!("other:{:?}", std::mem::discriminant(other)),
        };
        self.seen.lock().unwrap().push(label);
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn a08_1_event_fires_with_payload_notify() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(ProbeExt { id: "probe".into(), seen: seen.clone() }))
        .await
        .unwrap();

    let sub = host.subscriber();
    sub.on_event(&AgentEvent::AgentStart, CancelToken::new()).await;
    sub.on_event(&AgentEvent::ToolExecutionEnd {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        result: json!({"ok": true}),
        is_error: false,
    }, CancelToken::new())
    .await;
    // An event the probe did NOT subscribe to must not be delivered.
    sub.on_event(&AgentEvent::TurnStart, CancelToken::new()).await;

    let got = seen.lock().unwrap().clone();
    assert_eq!(got, vec!["agent_start", "tool_exec_end:tc1:false"]);
}

// ---------------------------------------------------------------------------
// A-08-1b: turn_start/turn_end carry the DERIVED turn index (Pi `_turnIndex`,
// agent-session.ts:615-635) — the `ExtSubscriber` fan-out layer maintains the counter even though
// the upstream `AgentEvent::{TurnStart,TurnEnd}` are payload-less. The index resets on agent_start,
// is shared by a turn's start+end, increments after each turn_end, and turn_start carries a
// non-zero wall-clock timestamp (Pi `Date.now()`).
// ---------------------------------------------------------------------------
/// `(label, turn_index, timestamp)` recorded for each delivered turn event.
type TurnLog = Arc<Mutex<Vec<(&'static str, u32, u64)>>>;

struct TurnProbe {
    id: ExtensionId,
    seen: TurnLog,
}

#[async_trait::async_trait]
impl NativeExtension for TurnProbe {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::TurnStart, EventKind::TurnEnd]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::TurnStart { turn_index, timestamp } => {
                self.seen.lock().unwrap().push(("turn_start", *turn_index, *timestamp));
            }
            HostEvent::TurnEnd { turn_index, .. } => {
                self.seen.lock().unwrap().push(("turn_end", *turn_index, 0));
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn a08_1b_turn_index_is_derived_and_resets_per_agent_run() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(TurnProbe { id: "turns".into(), seen: seen.clone() }))
        .await
        .unwrap();

    let sub = host.subscriber();

    // Two turns, then a fresh agent run that resets the index, then one more turn. The probe is NOT
    // subscribed to agent_start, but the counter must still reset (the gate runs AFTER the counter).
    let turn_end = || {
        let (msg, _tc) = tool_call_msg("read", &"tc".into(), &json!({}));
        AgentEvent::TurnEnd { message: AgentMessage::Assistant(Arc::new(msg)), tool_results: vec![] }
    };
    sub.on_event(&AgentEvent::AgentStart, CancelToken::new()).await;
    sub.on_event(&AgentEvent::TurnStart, CancelToken::new()).await;
    sub.on_event(&turn_end(), CancelToken::new()).await;
    sub.on_event(&AgentEvent::TurnStart, CancelToken::new()).await;
    sub.on_event(&turn_end(), CancelToken::new()).await;
    sub.on_event(&AgentEvent::AgentStart, CancelToken::new()).await; // reset back to 0
    sub.on_event(&AgentEvent::TurnStart, CancelToken::new()).await;

    let got = seen.lock().unwrap().clone();
    // start+end of a turn share the index; it increments after each end; agent_start resets it.
    let indices: Vec<(&str, u32)> = got.iter().map(|(l, i, _)| (*l, *i)).collect();
    assert_eq!(
        indices,
        vec![
            ("turn_start", 0),
            ("turn_end", 0),
            ("turn_start", 1),
            ("turn_end", 1),
            ("turn_start", 0), // reset by the second agent_start
        ]
    );
    // Every turn_start carries a real wall-clock timestamp (Pi `Date.now()`), never 0.
    for (label, _, ts) in &got {
        if *label == "turn_start" {
            assert!(*ts > 0, "turn_start timestamp must be a real wall-clock ms value");
        }
    }
}

// ---------------------------------------------------------------------------
// A-08-2: tool_call blocks a bash call with a reason (R-08-010).
// ---------------------------------------------------------------------------
struct BashGate {
    id: ExtensionId,
}

#[async_trait::async_trait]
impl NativeExtension for BashGate {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::ToolCall { name, .. } = ev
            && name == "bash" {
                return HookOutcome::Block { reason: Some("bash is not allowed".into()), terminate: TerminateHint::Unspecified };
            }
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn a08_2_tool_call_blocks_bash_with_reason() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(BashGate { id: "gate".into() })).await.unwrap();
    let hooks = host.hooks();

    let mut args = json!({"command": "rm -rf /"});
    let id: ToolCallId = "tc1".into();
    let (msg, tc) = tool_call_msg("bash", &id, &args);
    let ctx = BeforeToolCall {
        tool_name: "bash",
        tool_call_id: &id,
        args: &mut args,
        messages: &[],
        assistant_message: &msg,
        tool_call: &tc,
        context: empty_view(),
    };
    let outcome = hooks.before_tool_call(ctx, CancelToken::new()).await;
    match outcome {
        BeforeOutcome::Block { reason, .. } => assert_eq!(reason.as_deref(), Some("bash is not allowed")),
        BeforeOutcome::Proceed | BeforeOutcome::Failed(_) => panic!("expected block"),
    }

    // A non-bash tool proceeds untouched.
    let mut a2 = json!({"path": "x"});
    let (msg2, tc2) = tool_call_msg("read", &id, &a2);
    let ctx2 = BeforeToolCall {
        tool_name: "read",
        tool_call_id: &id,
        args: &mut a2,
        messages: &[],
        assistant_message: &msg2,
        tool_call: &tc2,
        context: empty_view(),
    };
    assert!(matches!(
        hooks.before_tool_call(ctx2, CancelToken::new()).await,
        BeforeOutcome::Proceed
    ));
}

// before_tool_call mutate: rewrite the input; later handler observes the folded value.
struct ArgMutator {
    id: ExtensionId,
    key: String,
    val: String,
}

#[async_trait::async_trait]
impl NativeExtension for ArgMutator {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::ToolCall { input, .. } = ev {
            let mut next = input.clone();
            if let Value::Object(map) = &mut next {
                map.insert(self.key.clone(), Value::String(self.val.clone()));
            }
            return HookOutcome::Mutate(crate::EventPatch::ToolInput(next));
        }
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn tool_call_mutate_chains_in_load_order() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(ArgMutator { id: "m1".into(), key: "a".into(), val: "1".into() }))
        .await
        .unwrap();
    host.load_native(Arc::new(ArgMutator { id: "m2".into(), key: "b".into(), val: "2".into() }))
        .await
        .unwrap();
    let hooks = host.hooks();

    let mut args = json!({});
    let id: ToolCallId = "tc1".into();
    let (msg, tc) = tool_call_msg("x", &id, &args);
    let ctx = BeforeToolCall {
        tool_name: "x",
        tool_call_id: &id,
        args: &mut args,
        messages: &[],
        assistant_message: &msg,
        tool_call: &tc,
        context: empty_view(),
    };
    let out = hooks.before_tool_call(ctx, CancelToken::new()).await;
    assert!(matches!(out, BeforeOutcome::Proceed));
    assert_eq!(args, json!({"a": "1", "b": "2"}));
}

// ---------------------------------------------------------------------------
// A-08-3: tool_result patch chains (R-08-011).
// ---------------------------------------------------------------------------
struct ResultAppender {
    id: ExtensionId,
    suffix: String,
}

#[async_trait::async_trait]
impl NativeExtension for ResultAppender {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::ToolResult { content, .. } = ev {
            // Observe the FOLDED content and append (chaining).
            let mut text = String::new();
            for c in content {
                if let Content::Text { text: t, .. } = c {
                    text.push_str(t);
                }
            }
            text.push_str(&self.suffix);
            return HookOutcome::Mutate(crate::EventPatch::ToolResult {
                content: Some(vec![Content::text(text)]),
                details: None,
                is_error: None,
                usage: None,
                terminate: None,
            });
        }
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn a08_3_tool_result_patch_chains() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(ResultAppender { id: "r1".into(), suffix: "-A".into() }))
        .await
        .unwrap();
    host.load_native(Arc::new(ResultAppender { id: "r2".into(), suffix: "-B".into() }))
        .await
        .unwrap();
    let hooks = host.hooks();

    let id: ToolCallId = "tc1".into();
    let content = vec![Content::text("base")];
    let args = json!({});
    let (msg, tc) = tool_call_msg("x", &id, &args);
    let ctx = AfterToolCall {
        tool_name: "x",
        tool_call_id: &id,
        args: &args,
        content: &content,
        details: None,
        usage: None,
        is_error: false,
        terminate: TerminateHint::Continue,
        assistant_message: &msg,
        tool_call: &tc,
        context: empty_view(),
    };
    let over = match hooks.after_tool_call(ctx, CancelToken::new()).await {
        cyrup_agent::AfterOutcome::Override(o) => o,
        _ => panic!("expected override"),
    };
    let new = over.content.expect("content patched");
    match &new[0] {
        Content::Text { text, .. } => assert_eq!(text, "base-A-B"),
        _ => panic!("expected text"),
    }
}

// ---------------------------------------------------------------------------
// A-08-4: a registered tool overrides the built-in `read` (R-08-012/014).
// ---------------------------------------------------------------------------
struct FakeRead {
    schema: Value,
}
#[async_trait::async_trait]
impl Tool for FakeRead {
    fn name(&self) -> &str {
        "read"
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("EXTENSION-READ")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

struct BuiltinRead {
    schema: Value,
}
#[async_trait::async_trait]
impl Tool for BuiltinRead {
    fn name(&self) -> &str {
        "read"
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("BUILTIN-READ")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

struct ReadOverrideExt {
    id: ExtensionId,
}
#[async_trait::async_trait]
impl NativeExtension for ReadOverrideExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_tool(Arc::new(FakeRead { schema: json!({"type": "object"}) }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn a08_4_registered_tool_overrides_builtin_read() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(ReadOverrideExt { id: "override".into() })).await.unwrap();

    let builtins: Vec<Arc<dyn Tool>> =
        vec![Arc::new(BuiltinRead { schema: json!({"type": "object"}) })];
    let active = host.active_tools(&builtins).unwrap();
    assert_eq!(active.len(), 1, "override does not add a second `read`");
    let read = active.iter().find(|t| t.name() == "read").unwrap();

    let mut updates: Vec<cyrup_core::ToolUpdate> = Vec::new();
    let sink: cyrup_core::ToolUpdateSink = Box::new(move |u| updates.push(u));
    let res = read
        .execute("tc1".into(), json!({}), CancelToken::new(), sink)
        .await
        .unwrap();
    match &res.content[0] {
        Content::Text { text, .. } => assert_eq!(text, "EXTENSION-READ"),
        _ => panic!("expected text"),
    }
}

// ---------------------------------------------------------------------------
// A-08-5 (subset): context hook filters messages.
// ---------------------------------------------------------------------------
struct DropAssistant {
    id: ExtensionId,
}
#[async_trait::async_trait]
impl NativeExtension for DropAssistant {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::Context]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::Context { messages } = ev {
            let filtered: Vec<Arc<AgentMessage>> =
                messages.iter().filter(|m| !m.is_assistant()).cloned().collect();
            return HookOutcome::Mutate(crate::EventPatch::Context { messages: filtered });
        }
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn a08_5_context_hook_filters_messages() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(DropAssistant { id: "ctx".into() })).await.unwrap();
    let hooks = host.hooks();

    let msgs = vec![
        AgentMessage::user_text("hi"),
        AgentMessage::Assistant(Arc::new(cyrup_core::AssistantMessage::errored(
            "faux".into(),
            "m",
            Some("faux".into()),
            cyrup_core::StopReason::Stop,
            "x",
        ))),
        AgentMessage::user_text("bye"),
    ];
    let out = hooks.transform_context(msgs.into_iter().map(Arc::new).collect(), CancelToken::new()).await.unwrap();
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|m| !m.is_assistant()));
}

// ---------------------------------------------------------------------------
// R-08-034: subscription-gated near-zero dispatch when no subscriber.
// ---------------------------------------------------------------------------
struct CountingExt {
    id: ExtensionId,
    calls: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl NativeExtension for CountingExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::AgentStart]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn r08_034_subscription_gated_dispatch() {
    let host = ExtensionHost::new(cfg());
    let calls = Arc::new(AtomicUsize::new(0));
    host.load_native(Arc::new(CountingExt { id: "c".into(), calls: calls.clone() }))
        .await
        .unwrap();

    // Nobody subscribes to ToolExecUpdate -> the cheap gate returns true (single bitset test).
    assert!(host.dispatcher().no_subscribers(EventKind::ToolExecUpdate));
    assert!(!host.dispatcher().no_subscribers(EventKind::AgentStart));

    let sub = host.subscriber();
    // High-frequency event with no subscriber: handler not invoked.
    sub.on_event(&AgentEvent::ToolExecutionUpdate {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        args: json!({}),
        partial_result: json!({}),
    }, CancelToken::new())
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    sub.on_event(&AgentEvent::AgentStart, CancelToken::new()).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// R-08-036: a panicking native handler is contained (host never crashes).
// ---------------------------------------------------------------------------
struct PanicExt {
    id: ExtensionId,
}
#[async_trait::async_trait]
impl NativeExtension for PanicExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        panic!("boom inside extension");
    }
}

#[tokio::test]
async fn r08_036_panicking_handler_is_contained() {
    let host = ExtensionHost::new(cfg());
    // Panicking gate first, then a real gate after it: the chain must continue past the panic.
    host.load_native(Arc::new(PanicExt { id: "panic".into() })).await.unwrap();
    host.load_native(Arc::new(BashGate { id: "gate".into() })).await.unwrap();
    let hooks = host.hooks();

    let mut args = json!({});
    let id: ToolCallId = "tc1".into();
    let (msg, tc) = tool_call_msg("bash", &id, &args);
    let ctx = BeforeToolCall {
        tool_name: "bash",
        tool_call_id: &id,
        args: &mut args,
        messages: &[],
        assistant_message: &msg,
        tool_call: &tc,
        context: empty_view(),
    };
    // The panic is caught (host alive) and, on the `tool_call` seam, blocks in its own right
    // (EXT-001) — the chain does not even need to reach the later real gate.
    let out = hooks.before_tool_call(ctx, CancelToken::new()).await;
    assert!(matches!(out, BeforeOutcome::Block { .. }));
}

// ---------------------------------------------------------------------------
// R-08-036 + Pi `onError`: a contained fault is surfaced to registered error listeners with the
// typed {extension, event, error} payload (ExtensionError listener registry).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn error_listener_captures_contained_fault() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PanicExt { id: "panic".into() })).await.unwrap();

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    host.add_error_listener(Arc::new(move |e: &ExtensionError| {
        if let Ok(mut g) = sink.lock() {
            g.push((e.extension.to_string(), e.event.to_string()));
        }
    }));

    // The panicking handler faults; the listener is notified AND — because `tool_call` fails CLOSED
    // (EXT-001, pi's uncaught `emitToolCall`, runner.ts:932-953) — the call is blocked, not passed.
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall { call_id: "t".into(), name: "bash".into(), input: json!({}) },
            &CancelToken::new(),
        )
        .await;
    assert!(
        matches!(reduced, Reduced::Blocked { .. }),
        "fault on the permission seam blocks the action (fail CLOSED), got {reduced:?}"
    );

    let got = captured.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one fault captured");
    assert_eq!(got[0].0, "panic", "attributed to the faulting extension");
    assert_eq!(got[0].1, "tool_call", "carries the event name (Pi ExtensionError.event)");
}

// ---------------------------------------------------------------------------
// R-08-036: a cooperatively-looping handler is contained by the invocation budget.
// ---------------------------------------------------------------------------
struct LoopExt {
    id: ExtensionId,
}
#[async_trait::async_trait]
impl NativeExtension for LoopExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::AgentStart]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        loop {
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::test]
async fn r08_036_looping_handler_is_budget_contained() {
    use crate::{Dispatcher, NativeHandle, Subscriptions};
    use std::time::{Duration, Instant};

    let subs = Subscriptions::empty().with(EventKind::AgentStart);
    let ctx = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    let handle = Arc::new(NativeHandle::new(Arc::new(LoopExt { id: "loop".into() }), subs, ctx));

    let dispatcher = Dispatcher::with_budget(Duration::from_millis(80));
    dispatcher.add(handle).unwrap();

    let start = Instant::now();
    dispatcher.dispatch_notify(&HostEvent::AgentStart, &CancelToken::new()).await;
    let elapsed = start.elapsed();
    // Returned (host alive) shortly after the budget, not hung.
    assert!(elapsed < Duration::from_secs(2), "dispatch should be budget-contained, took {elapsed:?}");
}

// ---------------------------------------------------------------------------
// R-08-016/017: command + shortcut registration via init.
// ---------------------------------------------------------------------------
struct CmdExt {
    id: ExtensionId,
}
#[async_trait::async_trait]
impl NativeExtension for CmdExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_command(
            "todo",
            CommandDescriptor { description: "manage todos".into(), completions: vec![] },
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn command_registration_visible() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(CmdExt { id: "cmd".into() })).await.unwrap();
    assert!(host.registry().has_command("todo").unwrap());
    assert!(!host.registry().has_command("missing").unwrap());
}

// ---------------------------------------------------------------------------
// Duplicate id is rejected with a typed error (never a panic).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn duplicate_id_rejected() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(CmdExt { id: "dup".into() })).await.unwrap();
    let err = host.load_native(Arc::new(CmdExt { id: "dup".into() })).await.unwrap_err();
    assert!(matches!(err, crate::ExtError::DuplicateId(_)));
}

// ---------------------------------------------------------------------------
// Deadlock rule (R-08-008): control ops are illegal from an event-tier ctx.
// ---------------------------------------------------------------------------
#[test]
fn r08_008_deadlock_guard_on_event_tier() {
    let ev = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    assert!(matches!(ev.require_command_tier(), Err(crate::ExtError::Deadlock)));
    let cmd = HostCtx::command(ExtMode::Tui, true, std::path::PathBuf::from("."));
    assert!(cmd.require_command_tier().is_ok());
}

// ---------------------------------------------------------------------------
// P-3 (permission-system-port §4): the dispatch-budget FORGIVENESS for a sanctioned human wait.
// A `before_tool_call` handler that holds a `HostCtx::begin_human_wait()` guard across a wait LONGER
// than the invocation budget must NOT be timed out (it would fail-OPEN a permission gate); a handler
// that blocks the SAME duration WITHOUT the guard is still budget-contained (unchanged fail-fast).
// ---------------------------------------------------------------------------
use std::time::Duration;

/// Holds a P-3 human-wait guard across a pure-async wait `wait` longer than the budget, then Blocks.
struct HumanGateExt {
    id: ExtensionId,
    wait: Duration,
}
#[async_trait::async_trait]
impl NativeExtension for HumanGateExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        // Enter a sanctioned human wait: the dispatch budget is suspended while the guard is held.
        let _human_wait = ctx.begin_human_wait();
        tokio::time::sleep(self.wait).await; // a "slow human" — longer than the budget
        HookOutcome::Block { reason: Some("human rejected".to_string()), terminate: TerminateHint::Unspecified }
    }
}

/// Blocks the same duration WITHOUT any human-wait guard (a cooperative runaway).
struct SlowNoGateExt {
    id: ExtensionId,
    wait: Duration,
}
#[async_trait::async_trait]
impl NativeExtension for SlowNoGateExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        tokio::time::sleep(self.wait).await;
        HookOutcome::Block { reason: Some("should never be observed (budget-timed-out)".to_string()), terminate: TerminateHint::Unspecified }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p3_human_wait_forgives_the_dispatch_budget() {
    use crate::{Dispatcher, NativeHandle, Subscriptions};

    let subs = Subscriptions::empty().with(EventKind::ToolCall);
    // A native handler ctx carries the shared HumanWaitGate the dispatcher consults.
    let ctx = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    // The handler waits 400ms — WAY past the 80ms budget — while holding the human-wait guard.
    let handle = Arc::new(NativeHandle::new(
        Arc::new(HumanGateExt { id: "human-gate".into(), wait: Duration::from_millis(400) }),
        subs,
        ctx,
    ));

    let dispatcher = Dispatcher::with_budget(Duration::from_millis(80));
    dispatcher.add(handle).unwrap();

    let ev = HostEvent::ToolCall {
        call_id: "c1".into(),
        name: "bash".into(),
        input: json!({ "command": "rm -rf /" }),
    };
    let reduced = dispatcher.dispatch_block_mutate(ev, &CancelToken::new()).await;
    // FORGIVEN: the slow human decision is honored (Blocked), NOT skipped-and-passed (fail-open).
    match reduced {
        Reduced::Blocked { reason, .. } => {
            assert_eq!(reason.as_deref(), Some("human rejected"), "the human decision reached the gate");
        }
        other => panic!("expected the slow human decision to Block, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p3_no_human_wait_is_still_budget_contained() {
    use crate::{Dispatcher, NativeHandle, Subscriptions};

    let subs = Subscriptions::empty().with(EventKind::ToolCall);
    let ctx = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    // Same 400ms wait, but NO human-wait guard: a cooperative runaway → budget-contained + skipped.
    let handle = Arc::new(NativeHandle::new(
        Arc::new(SlowNoGateExt { id: "slow-no-gate".into(), wait: Duration::from_millis(400) }),
        subs,
        ctx,
    ));

    let dispatcher = Dispatcher::with_budget(Duration::from_millis(80));
    dispatcher.add(handle).unwrap();

    let ev = HostEvent::ToolCall {
        call_id: "c2".into(),
        name: "bash".into(),
        input: json!({ "command": "echo hi" }),
    };
    let reduced = dispatcher.dispatch_block_mutate(ev, &CancelToken::new()).await;
    // EXT-032: the wall-clock assertion that used to sit here
    // (`assert!(elapsed < Duration::from_millis(300), …)`, a 220ms margin on an 80ms budget
    // against a 400ms sleep) is GONE. It could only fail spuriously: it pinned the scheduler's
    // interleaving under a full parallel run, and it proved nothing the deterministic assertion
    // below does not already prove — the "budget never fired" case is rejected outright by
    // requiring the reason to be pi's blocking-fault text, which a completed 400ms handler could
    // never produce.
    //
    // The runaway is contained at the budget (not its full 400ms wait). Because this is the
    // `tool_call` seam, containment fails CLOSED (EXT-001): the undecided gate DENIES rather than
    // silently allowing the call it was meant to gate.
    match reduced {
        Reduced::Blocked { reason, .. } => assert!(
            reason.as_deref().unwrap_or_default().contains("Extension failed, blocking execution"),
            "the budget timeout is reported as pi's blocking fault: {reason:?}"
        ),
        other => panic!("a budget-timed-out tool_call handler must Block, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// EXT-002: `message_end` is dispatched EXACTLY ONCE per finalized message.
//
// Pi has a single implementation (`ExtensionRunner.emitMessageEnd`, runner.ts:835) with a single
// caller (agent-session.ts:752), and `MessageEndEvent` is explicitly excluded from the generic
// `emit()` union (`RunnerEmitEvent`, runner.ts:124-137) — so a guest's `message_end` handler runs
// once per finalized message.
//
// cyrup wires the SAME agent to two subscribers (`cyrup-session-svc/src/builder.rs`):
// the notify-only `ExtSubscriber` and `SvcSubscriber`, and the latter re-dispatches the message
// through the mutating `ExtensionHost::emit_message_end` facade. This test drives both seams for
// one finalized message — exactly what production does — and asserts the handler saw it once.
// The `message_start` counter guards the other direction: suppressing the notify path for
// `message_end` must not silence the notify seam generally, nor the mutating seam itself.
// ---------------------------------------------------------------------------
struct MessageEndCounter {
    id: ExtensionId,
    ends: Arc<AtomicUsize>,
    starts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for MessageEndCounter {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::MessageEnd, EventKind::MessageStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::MessageEnd { .. } => {
                self.ends.fetch_add(1, Ordering::SeqCst);
            }
            HostEvent::MessageStart { .. } => {
                self.starts.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}

#[tokio::test]
async fn ext002_message_end_handler_runs_once_per_finalized_message() {
    let host = ExtensionHost::new(cfg());
    let ends = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    host.load_native(Arc::new(MessageEndCounter {
        id: "counter".into(),
        ends: ends.clone(),
        starts: starts.clone(),
    }))
    .await
    .unwrap();

    let cancel = CancelToken::new();
    let sub = host.subscriber();
    let agent_msg = AgentMessage::user_text("hello");

    // Seam 1 — the notify subscriber attached at builder.rs (`agent.subscribe(ext_subscriber)`).
    sub.on_event(&AgentEvent::MessageStart { message: agent_msg.clone() }, CancelToken::new()).await;
    sub.on_event(&AgentEvent::MessageEnd { message: agent_msg.clone() }, CancelToken::new()).await;
    // Seam 2 — `SvcSubscriber`'s re-dispatch of the SAME finalized message through the mutating
    // facade (`cyrup-session-svc/src/subscriber.rs`).
    let core = cyrup_core::Message::User { content: vec![Content::text("hello")], timestamp: 0 };
    assert!(
        host.emit_message_end(core, &cancel).await.is_none(),
        "a Noop handler leaves the message unmodified"
    );

    assert_eq!(
        ends.load(Ordering::SeqCst),
        1,
        "message_end must reach a subscribed handler exactly once per finalized message (Pi \
         emitMessageEnd is the single dispatch point)"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 1, "the notify seam still delivers other kinds");
}

// ---------------------------------------------------------------------------
// EXT-034: a `pi.events` emit from an EVENT handler is delivered.
// ---------------------------------------------------------------------------

/// Emits on the inter-extension bus from inside its `message_start` EVENT handler — the seam
/// upstream delivers inline (`createEventBus().emit` runs every listener at the emit call,
/// `pi/packages/coding-agent/src/core/event-bus.ts:12-32` @v0.83.0) and cyrup used to drain only in
/// the command tier.
struct BusEmitterOnEvent {
    bus: Arc<crate::bus::SharedBus>,
}

#[async_trait::async_trait]
impl NativeExtension for BusEmitterOnEvent {
    fn id(&self) -> ExtensionId {
        "emitter".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::MessageStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::MessageStart { .. }) {
            self.bus.emit("demo:bus".into(), json!({"from": "event-handler"}));
        }
        HookOutcome::Noop
    }
}

/// Listens on `demo:bus` and records every payload it is handed.
struct BusListener {
    seen: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl NativeExtension for BusListener {
    fn id(&self) -> ExtensionId {
        "listener".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe_bus("demo:bus");
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &Value,
        ctx: &HostCtx,
    ) -> Result<(), crate::ExtError> {
        assert_eq!(topic, "demo:bus");
        // A bus listener is not a command: it must run at the EVENT tier.
        assert_eq!(ctx.tier(), crate::native::CtxTier::Event, "bus delivery uses an event-tier ctx");
        self.seen.lock().unwrap().push(payload.clone());
        Ok(())
    }
}

/// EXT-034 — the drain used to be wired into `run_command`/`run_shortcut` only, so `pi.events`
/// silently worked from a slash-command handler and silently did NOT work from an event handler,
/// which is where cross-extension coordination actually happens (a permission decision, a tool
/// result, a session start). Upstream there is no drain point at all: `emit` runs every listener
/// synchronously inside the emit call, so an emit can never go undelivered.
///
/// No manual `deliver_bus_events` here — that is the whole assertion.
#[tokio::test]
async fn ext034_bus_emit_from_an_event_handler_is_delivered_without_a_manual_drain() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(BusListener { seen: seen.clone() })).await.unwrap();
    host.load_native(Arc::new(BusEmitterOnEvent { bus: host.bus().clone() })).await.unwrap();

    let sub = host.subscriber();
    sub.on_event(&AgentEvent::MessageStart { message: AgentMessage::user_text("hi") }, CancelToken::new())
        .await;

    let got = seen.lock().unwrap().clone();
    assert_eq!(
        got.len(),
        1,
        "an emit from an event handler must be delivered when the dispatch returns, not left \
         queued until the next slash command"
    );
    assert_eq!(got[0], json!({"from": "event-handler"}));
    assert_eq!(host.bus().pending_len(), 0, "the queue is empty after the dispatch seam drains");
}

/// The same emit on the block/mutate seam (`tool_call` — the permission gate's own event), whose
/// first-block short-circuit returns before the end of the subscriber loop. That early return is
/// the one an EXT-034 fix is most likely to miss, and it is the coordination case that matters:
/// a gate announcing the decision it just made.
struct BusEmitterOnBlock {
    bus: Arc<crate::bus::SharedBus>,
}

#[async_trait::async_trait]
impl NativeExtension for BusEmitterOnBlock {
    fn id(&self) -> ExtensionId {
        "gate".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        self.bus.emit("demo:bus".into(), json!({"from": "gate"}));
        HookOutcome::Block { reason: Some("denied".into()), terminate: TerminateHint::Unspecified }
    }
}

#[tokio::test]
async fn ext034_bus_emit_survives_the_first_block_short_circuit() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(BusListener { seen: seen.clone() })).await.unwrap();
    host.load_native(Arc::new(BusEmitterOnBlock { bus: host.bus().clone() })).await.unwrap();

    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "tc-1".into(),
                name: "bash".into(),
                input: json!({"command": "rm -rf /"}),
            },
            &CancelToken::new(),
        )
        .await;
    assert!(matches!(reduced, Reduced::Blocked { .. }), "the gate blocked");
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "a gate that emits AND blocks must still have its emit delivered — pi's emit is inline, so \
         the listener ran before the block was even returned"
    );
}

// ---------------------------------------------------------------------------
// EXT-035: a native's shortcut/flag/provider registrations are reachable.
// ---------------------------------------------------------------------------

/// Registers one of each of the surfaces `InitApi` did not offer, and records the shortcut firing.
struct FullRegistrar {
    fired: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for FullRegistrar {
    fn id(&self) -> ExtensionId {
        "registrar".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_shortcut("ctrl+alt+f", Some("Show the fleet".into()));
        api.register_flag("fleet", json!({"type": "boolean", "description": "fleet mode"}));
        // pi's `ProviderConfig.models` is `ProviderModelConfig[]` — OBJECTS carrying
        // `id`/`name`/`reasoning`/`input`/`cost`/`contextWindow`/`maxTokens`, never bare id strings
        // (`pi/packages/coding-agent/src/core/extensions/types.ts:1443` and `:1467-1492` @v0.83.0),
        // and `ProviderHub::register` parses the typed shape.
        api.register_provider(
            "fleet-provider",
            json!({
                "baseUrl": "https://fleet.example.com",
                "api": "openai-completions",
                "models": [{
                    "id": "fleet-1",
                    "name": "Fleet One",
                    "reasoning": false,
                    "input": ["text"],
                    "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
                    "contextWindow": 200_000,
                    "maxTokens": 8_192
                }]
            }),
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn execute_shortcut(&self, key: &str, ctx: &HostCtx) -> Result<(), crate::ExtError> {
        assert_eq!(key, "ctrl+alt+f");
        // pi hands a shortcut handler the same `ExtensionContext` a command handler gets
        // (`extensions/types.ts:1249-1255` @v0.83.0), so session-replacing ops must be permitted.
        assert_eq!(ctx.tier(), crate::native::CtxTier::Command);
        self.fired.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// EXT-035 — `run_shortcut` was `#[cfg(feature = "wasm-host")]` and resolved owners out of the
/// live-WASM map only, so a native-owned shortcut registered, was advertised by `shortcut_keys()`
/// and listed by `/hotkeys`, and then could never fire. pi has ONE extension kind and one
/// `ExtensionAPI` (`extensions/loader.ts:274-410` @v0.83.0): there is no upstream notion of an
/// extension that may register a shortcut but not have it invoked.
#[tokio::test]
async fn ext035_a_native_registered_shortcut_actually_fires() {
    let host = ExtensionHost::new(cfg());
    let fired = Arc::new(AtomicUsize::new(0));
    host.load_native(Arc::new(FullRegistrar { fired: fired.clone() })).await.unwrap();

    assert!(
        host.shortcut_keys().contains(&"ctrl+alt+f".to_string()),
        "the key is advertised to the TUI"
    );
    assert_eq!(
        host.shortcut_specs()
            .into_iter()
            .find(|(k, _)| k == "ctrl+alt+f")
            .and_then(|(_, d)| d)
            .as_deref(),
        Some("Show the fleet"),
        "EXT-040: /hotkeys renders `shortcut.description ?? extensionPath`, never the key id"
    );

    host.run_shortcut("ctrl+alt+f", &CancelToken::new()).await.expect("the native handler runs");
    assert_eq!(fired.load(Ordering::SeqCst), 1);

    // The other two EXT-035 surfaces reached the registry from the same `init`.
    assert!(host.registry().provider_ids().unwrap().contains(&"fleet-provider".to_string()));
    assert!(host.registry().get_flag("fleet").unwrap().is_some());
}

/// An unregistered key is still a typed error, and a native with no `execute_shortcut` override
/// reports one rather than succeeding silently.
#[tokio::test]
async fn ext035_an_unhandled_shortcut_is_a_typed_error_not_a_silent_success() {
    let host = ExtensionHost::new(cfg());
    let err = host.run_shortcut("ctrl+q", &CancelToken::new()).await.unwrap_err();
    assert!(format!("{err}").contains("no such shortcut"), "{err}");
}

// ---------------------------------------------------------------------------
// EXT-019: registerMarkdownTransformer (pi v0.84.1).
// ---------------------------------------------------------------------------

/// Appends its own marker so the FOLD order is observable in the output.
struct MarkTransformer {
    id: &'static str,
    marker: &'static str,
}

#[async_trait::async_trait]
impl NativeExtension for MarkTransformer {
    fn id(&self) -> ExtensionId {
        self.id.into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_markdown_transformer();
        // A second call must be idempotent — upstream ASSIGNS `extension.markdownTransformer`
        // (`extensions/loader.ts:309-312` @v0.84.1), so an extension has at most one fold step.
        api.register_markdown_transformer();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn transform_markdown(&self, markdown: &str, ctx: &Value) -> String {
        format!(
            "{markdown}{}[{}|{}|{}]",
            self.marker,
            ctx["messageType"].as_str().unwrap_or("?"),
            ctx["isStreaming"].as_bool().unwrap_or(false),
            ctx["availableWidth"].as_u64().unwrap_or(0),
        )
    }
}

/// EXT-019 — pi's `getMarkdownTransformers()` is
/// `this.extensions.flatMap(ext => ext.markdownTransformer ? [..] : [])`
/// (`pi/packages/coding-agent/src/core/extensions/runner.ts:589-591` @v0.84.1), so the fold order
/// IS extension load order and each transformer's output feeds the next.
#[tokio::test]
async fn ext019_markdown_transformers_fold_in_load_order_with_pis_context_fields() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(MarkTransformer { id: "first", marker: "-A" })).await.unwrap();
    host.load_native(Arc::new(MarkTransformer { id: "second", marker: "-B" })).await.unwrap();

    let out = host.transform_markdown("body", "assistant-thinking", true, 80).await;
    assert_eq!(
        out, "body-A[assistant-thinking|true|80]-B[assistant-thinking|true|80]",
        "each transformer's output is the next one's input, in LOAD order"
    );

    // The three `MarkdownTransformContext` fields (`extensions/types.ts:1147-1151`) really vary.
    let settled = host.transform_markdown("x", "user", false, 40).await;
    assert!(settled.contains("[user|false|40]"), "{settled}");
}

/// With no transformer registered the markdown is returned untouched and no fold runs — the
/// upstream `flatMap` over zero transformers.
#[tokio::test]
async fn ext019_no_registered_transformer_is_the_identity() {
    let host = ExtensionHost::new(cfg());
    assert_eq!(host.transform_markdown("as-is", "assistant", false, 10).await, "as-is");
}

/// A PANICKING transformer is contained and SKIPPED: its input passes through unchanged, and the
/// rest of the fold still runs. A presentation hook must never be able to blank a line of
/// transcript (R-08-036) — pi's transformers run inside the interactive renderer, where a throw is
/// caught at the component.
struct PanickingTransformer;

#[async_trait::async_trait]
impl NativeExtension for PanickingTransformer {
    fn id(&self) -> ExtensionId {
        "panics".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_markdown_transformer();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn transform_markdown(&self, _markdown: &str, _ctx: &Value) -> String {
        panic!("transformer bug");
    }
}

#[tokio::test]
async fn ext019_a_panicking_transformer_is_contained_and_the_text_survives() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PanickingTransformer)).await.unwrap();
    host.load_native(Arc::new(MarkTransformer { id: "after", marker: "-B" })).await.unwrap();

    let out = host.transform_markdown("keep", "assistant", false, 20).await;
    assert_eq!(
        out, "keep-B[assistant|false|20]",
        "the panicking step passes its input through untouched and the chain continues"
    );
}

// ---------------------------------------------------------------------------
// EXT-021 (residual): onTerminalInput.
// ---------------------------------------------------------------------------

/// Rewrites the input by appending its marker, or consumes it outright when it sees `"!"`.
struct InputWatcher {
    id: &'static str,
    marker: &'static str,
    consume_on_bang: bool,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for InputWatcher {
    fn id(&self) -> ExtensionId {
        self.id.into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe_terminal_input();
        // Idempotent, matching upstream's `Set.add`.
        api.subscribe_terminal_input();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_terminal_input(&self, data: &str) -> Option<crate::TerminalInputResult> {
        self.seen.lock().unwrap().push(data.to_string());
        if self.consume_on_bang && data.contains('!') {
            return Some(crate::TerminalInputResult { consume: Some(true), data: None });
        }
        Some(crate::TerminalInputResult {
            consume: None,
            data: Some(format!("{data}{}", self.marker)),
        })
    }
}

/// EXT-021's residual, closed. The crate-level CYRUP-DELTA register named `onTerminalInput`
/// (`extensions/types.ts:145` @v0.83.0) as the ONE member of `interface ui` that was an open GAP
/// rather than a delta, blocked only on the `HOST_WORLD` bump this pass spends.
///
/// The fold is pi's `TUI.handleInput` (`packages/tui/src/tui.ts:773-788`): each listener sees the
/// CURRENT (possibly rewritten) data, in insertion order.
#[tokio::test]
async fn ext021_terminal_input_handlers_fold_in_load_order_and_each_sees_the_rewritten_data() {
    let host = ExtensionHost::new(cfg());
    let first_seen = Arc::new(Mutex::new(Vec::new()));
    let second_seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(InputWatcher {
        id: "first",
        marker: "-A",
        consume_on_bang: false,
        seen: first_seen.clone(),
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(InputWatcher {
        id: "second",
        marker: "-B",
        consume_on_bang: false,
        seen: second_seen.clone(),
    }))
    .await
    .unwrap();

    let decision = host.terminal_input("k").await;
    assert_eq!(decision, crate::TerminalInputDecision::Deliver("k-A-B".into()));
    assert_eq!(first_seen.lock().unwrap().as_slice(), ["k"]);
    assert_eq!(
        second_seen.lock().unwrap().as_slice(),
        ["k-A"],
        "the second listener sees the FIRST one's rewrite, not the original (tui.ts:780-782)"
    );
}

/// With no subscriber the chunk is delivered untouched — upstream guards the whole fold on
/// `inputListeners.size > 0` (`tui.ts:773`). Asserted FIRST-CLASS rather than implied, because it
/// is the path every keystroke in an extension-less session takes.
#[tokio::test]
async fn ext021_no_subscriber_delivers_the_input_untouched() {
    let host = ExtensionHost::new(cfg());
    assert_eq!(
        host.terminal_input("\x1b[A").await,
        crate::TerminalInputDecision::Deliver("\x1b[A".into())
    );
}

/// `consume: true` STOPS the fold and drops the keystroke (`tui.ts:777-779`) — the listeners after
/// it never run.
#[tokio::test]
async fn ext021_consume_stops_the_fold_and_drops_the_keystroke() {
    let host = ExtensionHost::new(cfg());
    let first_seen = Arc::new(Mutex::new(Vec::new()));
    let second_seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(InputWatcher {
        id: "eater",
        marker: "-A",
        consume_on_bang: true,
        seen: first_seen.clone(),
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(InputWatcher {
        id: "after",
        marker: "-B",
        consume_on_bang: false,
        seen: second_seen.clone(),
    }))
    .await
    .unwrap();

    // A non-bang chunk still reaches both, so the assertion below is not vacuous.
    assert_eq!(host.terminal_input("q").await, crate::TerminalInputDecision::Deliver("q-A-B".into()));
    assert_eq!(second_seen.lock().unwrap().len(), 1);

    assert_eq!(host.terminal_input("!").await, crate::TerminalInputDecision::Consume);
    assert_eq!(
        second_seen.lock().unwrap().len(),
        1,
        "the listener after a consuming one must not run"
    );
}

/// A PANICKING handler is contained and treated as upstream's `undefined`: the input passes
/// through and the rest of the fold still runs. Failing CLOSED here would let one broken extension
/// swallow the keyboard with no way to type the command that unloads it.
struct PanickingWatcher;

#[async_trait::async_trait]
impl NativeExtension for PanickingWatcher {
    fn id(&self) -> ExtensionId {
        "panics-on-input".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe_terminal_input();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_terminal_input(&self, _data: &str) -> Option<crate::TerminalInputResult> {
        panic!("input handler bug");
    }
}

#[tokio::test]
async fn ext021_a_panicking_input_handler_is_contained_and_the_keystroke_survives() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(PanickingWatcher)).await.unwrap();
    host.load_native(Arc::new(InputWatcher {
        id: "after",
        marker: "-B",
        consume_on_bang: false,
        seen: Arc::new(Mutex::new(Vec::new())),
    }))
    .await
    .unwrap();

    assert_eq!(
        host.terminal_input("z").await,
        crate::TerminalInputDecision::Deliver("z-B".into()),
        "the panicking step passes its input through and the chain continues"
    );
}

/// A fold that ends with an EMPTY string also drops the keystroke (`tui.ts:784-786`) — distinct
/// from `consume`, and the reason `TerminalInputResult.data` is an `Option<String>` rather than a
/// bare `String`.
struct BlankingWatcher;

#[async_trait::async_trait]
impl NativeExtension for BlankingWatcher {
    fn id(&self) -> ExtensionId {
        "blanks".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe_terminal_input();
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_terminal_input(&self, _data: &str) -> Option<crate::TerminalInputResult> {
        Some(crate::TerminalInputResult { consume: None, data: Some(String::new()) })
    }
}

#[tokio::test]
async fn ext021_a_fold_that_ends_empty_drops_the_keystroke_without_consume() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(BlankingWatcher)).await.unwrap();
    assert_eq!(host.terminal_input("x").await, crate::TerminalInputDecision::Consume);
}

/// EXT-034, the re-entrancy half. A `Some(exclude)` on a dispatch means a guest is SUSPENDED inside
/// one of its own host imports (`provider-stream.on-payload`) holding its single-instance
/// `tokio::Mutex` store guard — the forced divergence documented on
/// `Dispatcher::dispatch_block_mutate_excluding`. Draining there would await the guard that guest
/// already holds and HANG, because a re-entrant `tokio::Mutex::lock` has no deadlock detection.
///
/// pi cannot hit this: its runner is one JS process, `createEventBus().emit` runs listeners inline,
/// and a re-entered handler is an ordinary nested call.
///
/// The events must therefore stay QUEUED across an excluded seam and go out at the next ordinary
/// one. Asserted with natives (which have no store to deadlock on) because the assertion is about
/// the dispatcher's rule, not about wasm: if the rule regresses, a wasm run hangs instead of
/// failing, and a hang is exactly what a test cannot report.
#[tokio::test]
async fn ext034_an_excluded_seam_defers_the_drain_instead_of_re_entering() {
    let host = ExtensionHost::new(cfg());
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(BusListener { seen: seen.clone() })).await.unwrap();

    // Queue an event as if a guest had emitted from inside its own import.
    host.bus().emit("demo:bus".into(), json!({"from": "suspended-guest"}));

    // An EXCLUDED dispatch must not drain: the excluded guest is mid-import.
    host.dispatcher()
        .dispatch_notify_excluding(
            &HostEvent::MessageStart { message: json!({"role": "user"}) },
            &CancelToken::new(),
            Some(&ExtensionId::from("suspended")),
        )
        .await;
    assert_eq!(
        seen.lock().unwrap().len(),
        0,
        "an excluded seam must leave the queue alone — draining there re-enters a held store"
    );
    assert_eq!(host.bus().pending_len(), 1, "the event is still queued, not dropped");

    // The next ORDINARY seam delivers it.
    host.dispatcher()
        .dispatch_notify(
            &HostEvent::MessageStart { message: json!({"role": "user"}) },
            &CancelToken::new(),
        )
        .await;
    assert_eq!(seen.lock().unwrap().len(), 1, "deferred, not lost");
}

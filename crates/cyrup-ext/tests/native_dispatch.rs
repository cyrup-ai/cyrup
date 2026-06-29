//! Native built-in dispatch/registration/seam/containment contracts (arch-08 §11). These drive the
//! FULL extension surface WITHOUT any wasm — native built-in extensions exercise every contract.
//! Maps to acceptance criteria A-08-1..5, R-08-034 (gated dispatch), and R-08-036 (containment).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::{
    AfterToolCall, AgentContextView, AgentEvent, AgentMessage, BeforeOutcome, BeforeToolCall,
};
use cyrup_core::{CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult};
use cyrup_ext::{
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
        arguments,
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
        error_message: None,
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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

    let sub = host.subscriber(CancelToken::new());
    sub.on_event(&AgentEvent::AgentStart).await;
    sub.on_event(&AgentEvent::ToolExecutionEnd {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        result: json!({"ok": true}),
        is_error: false,
    })
    .await;
    // An event the probe did NOT subscribe to must not be delivered.
    sub.on_event(&AgentEvent::TurnStart).await;

    let got = seen.lock().unwrap().clone();
    assert_eq!(got, vec!["agent_start", "tool_exec_end:tc1:false"]);
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::ToolCall { name, .. } = ev
            && name == "bash" {
                return HookOutcome::Block { reason: Some("bash is not allowed".into()) };
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
    let outcome = hooks.before_tool_call(ctx, CancelToken::new()).await.unwrap();
    match outcome {
        BeforeOutcome::Block { reason } => assert_eq!(reason.as_deref(), Some("bash is not allowed")),
        BeforeOutcome::Proceed => panic!("expected block"),
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
        hooks.before_tool_call(ctx2, CancelToken::new()).await.unwrap(),
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::ToolCall { input, .. } = ev {
            let mut next = input.clone();
            if let Value::Object(map) = &mut next {
                map.insert(self.key.clone(), Value::String(self.val.clone()));
            }
            return HookOutcome::Mutate(cyrup_ext::EventPatch::ToolInput(next));
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
    let out = hooks.before_tool_call(ctx, CancelToken::new()).await.unwrap();
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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
            return HookOutcome::Mutate(cyrup_ext::EventPatch::ToolResult {
                content: Some(vec![Content::text(text)]),
                details: None,
                is_error: None,
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
        is_error: false,
        terminate: false,
        assistant_message: &msg,
        tool_call: &tc,
        context: empty_view(),
    };
    let over = hooks.after_tool_call(ctx, CancelToken::new()).await.unwrap();
    let over = over.expect("expected override");
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
            terminate: false,
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
            terminate: false,
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[EventKind::Context]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::Context { messages } = ev {
            let filtered: Vec<AgentMessage> =
                messages.iter().filter(|m| !m.is_assistant()).cloned().collect();
            return HookOutcome::Mutate(cyrup_ext::EventPatch::Context { messages: filtered });
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
        AgentMessage::Assistant(cyrup_core::AssistantMessage::errored(
            "faux".into(),
            "m",
            Some("faux".into()),
            cyrup_core::StopReason::Stop,
            "x",
        )),
        AgentMessage::user_text("bye"),
    ];
    let out = hooks.transform_context(msgs, CancelToken::new()).await.unwrap();
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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

    let sub = host.subscriber(CancelToken::new());
    // High-frequency event with no subscriber: handler not invoked.
    sub.on_event(&AgentEvent::ToolExecutionUpdate {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        args: json!({}),
        partial_result: json!({}),
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    sub.on_event(&AgentEvent::AgentStart).await;
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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
    // The panic is caught and skipped; the later gate still blocks. Host alive.
    let out = hooks.before_tool_call(ctx, CancelToken::new()).await.unwrap();
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

    // The panicking handler faults; the chain proceeds (no block) and the listener is notified.
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall { call_id: "t".into(), name: "bash".into(), input: json!({}) },
            &CancelToken::new(),
        )
        .await;
    assert!(matches!(reduced, Reduced::Pass(_)), "fault skipped, action proceeds");

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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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
    use cyrup_ext::{Dispatcher, NativeHandle, Subscriptions};
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
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
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
    assert!(matches!(err, cyrup_ext::ExtError::DuplicateId(_)));
}

// ---------------------------------------------------------------------------
// Deadlock rule (R-08-008): control ops are illegal from an event-tier ctx.
// ---------------------------------------------------------------------------
#[test]
fn r08_008_deadlock_guard_on_event_tier() {
    let ev = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    assert!(matches!(ev.require_command_tier(), Err(cyrup_ext::ExtError::Deadlock)));
    let cmd = HostCtx::command(ExtMode::Tui, true, std::path::PathBuf::from("."));
    assert!(cmd.require_command_tier().is_ok());
}

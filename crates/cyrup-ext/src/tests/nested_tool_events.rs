//! CODE-006 — the extension events of a call another tool made carry its parent.
//!
//! pi puts `parentToolCallId` on the `tool_call`, `tool_result` and `tool_execution_*` events
//! (`core/extensions/types.ts:1061-1083`, `:1155-1161`, `:1231` @v1.0.1) and runs the nested call
//! through the SAME `tool_call` handlers a model-issued call goes through
//! (`agent-session.ts:629-690`), which is what keeps a permission gate from being routed around by
//! a tool that calls a tool. Native handlers read the parent from
//! [`HostCtx::parent_tool_call_id`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use cyrup_agent::{
    AfterOutcome, AfterToolCall, AgentContextView, AgentEvent, BeforeOutcome, BeforeToolCall,
    NestedToolExecutionEvent,
};
use cyrup_core::{CancelToken, Content, ExtensionId, TerminateHint, ToolCallId};
use serde_json::{Value, json};

use crate::{
    EventKind, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
    NativeExtension,
};

/// `(event kind, call id, parent)` per tool event, as the handler's ctx reports them.
type Seen = Arc<Mutex<Vec<(&'static str, String, Option<String>)>>>;

fn host() -> ExtensionHost {
    ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    })
}

/// Records, for every tool event it sees, the event kind, the call id and the parent the ctx
/// reports; blocks `bash` the way a permission gate does.
struct Recorder {
    seen: Seen,
}

#[async_trait::async_trait]
impl NativeExtension for Recorder {
    fn id(&self) -> ExtensionId {
        "recorder".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::ToolExecStart,
            EventKind::ToolExecUpdate,
            EventKind::ToolExecEnd,
        ]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        let (kind, call_id) = match ev {
            HostEvent::ToolCall { call_id, .. } => ("tool_call", call_id),
            HostEvent::ToolResult { call_id, .. } => ("tool_result", call_id),
            HostEvent::ToolExecStart { call_id, .. } => ("tool_execution_start", call_id),
            HostEvent::ToolExecUpdate { call_id, .. } => ("tool_execution_update", call_id),
            HostEvent::ToolExecEnd { call_id, .. } => ("tool_execution_end", call_id),
            _ => return HookOutcome::Noop,
        };
        self.seen.lock().unwrap().push((
            kind,
            call_id.to_string(),
            ctx.parent_tool_call_id().map(ToString::to_string),
        ));
        if let HostEvent::ToolCall { name, .. } = ev
            && name == "bash"
        {
            return HookOutcome::Block {
                reason: Some("bash is not allowed".into()),
                terminate: TerminateHint::Unspecified,
            };
        }
        HookOutcome::Noop
    }
}

async fn loaded() -> (ExtensionHost, Seen) {
    let host = host();
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.load_native(Arc::new(Recorder { seen: seen.clone() }))
        .await
        .unwrap();
    (host, seen)
}

fn assistant_and_call(
    name: &str,
    id: &ToolCallId,
) -> (cyrup_core::AssistantMessage, cyrup_core::ToolCall) {
    let call = cyrup_core::ToolCall {
        id: id.clone(),
        name: name.to_string(),
        arguments: serde_json::Map::new().into(),
        thought_signature: None,
        namespace: None,
    };
    let msg = cyrup_core::AssistantMessage {
        content: vec![Content::ToolCall(call.clone())],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage: cyrup_core::Usage::default(),
        stop_reason: cyrup_core::StopReason::ToolUse,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
        duration_ms: None,
    };
    (msg, call)
}

fn view() -> AgentContextView<'static> {
    AgentContextView {
        system_prompt: "",
        messages: &[],
        tools: &[],
    }
}

/// A `tool_call` made by another tool reaches the handler as the same event with the parent
/// readable, and is blocked by a gate exactly as a model-issued call is; the model-issued call
/// reports no parent.
#[tokio::test]
async fn tool_call_of_a_nested_call_carries_the_parent_and_is_gated() {
    let (host, seen) = loaded().await;
    let hooks = host.hooks();
    let parent: ToolCallId = "call".into();
    let nested: ToolCallId = "call/1".into();

    for (name, id, via_parent) in [
        ("read", &nested, true),
        ("bash", &nested, true),
        ("read", &parent, false),
    ] {
        let mut args = json!({});
        let (msg, call) = assistant_and_call(name, id);
        let ctx = BeforeToolCall {
            tool_name: name,
            tool_call_id: id,
            args: &mut args,
            messages: &[],
            assistant_message: &msg,
            tool_call: &call,
            context: view(),
        };
        let outcome = if via_parent {
            hooks
                .before_nested_tool_call(&parent, ctx, CancelToken::new())
                .await
        } else {
            hooks.before_tool_call(ctx, CancelToken::new()).await
        };
        match (name, outcome) {
            ("bash", BeforeOutcome::Block { reason, .. }) => {
                assert_eq!(reason.as_deref(), Some("bash is not allowed"));
            }
            ("bash", _) => panic!("the permission gate was routed around by a nested call"),
            (_, BeforeOutcome::Proceed) => {}
            _ => panic!("{name} should proceed"),
        }
    }

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("tool_call", "call/1".to_string(), Some("call".to_string())),
            ("tool_call", "call/1".to_string(), Some("call".to_string())),
            ("tool_call", "call".to_string(), None),
        ]
    );
}

/// `tool_result` of a nested call carries the parent too.
#[tokio::test]
async fn tool_result_of_a_nested_call_carries_the_parent() {
    let (host, seen) = loaded().await;
    let hooks = host.hooks();
    let parent: ToolCallId = "call".into();
    let nested: ToolCallId = "call/1".into();
    let (msg, call) = assistant_and_call("read", &nested);
    let args = json!({});
    let content = vec![Content::text("ok")];
    let ctx = AfterToolCall {
        tool_name: "read",
        tool_call_id: &nested,
        args: &args,
        content: &content,
        details: None,
        structured_content: None,
        usage: None,
        is_error: false,
        terminate: TerminateHint::Unspecified,
        assistant_message: &msg,
        tool_call: &call,
        context: view(),
    };
    let outcome = hooks
        .after_nested_tool_call(&parent, ctx, CancelToken::new())
        .await;
    assert!(matches!(outcome, AfterOutcome::Keep));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![(
            "tool_result",
            "call/1".to_string(),
            Some("call".to_string())
        )]
    );
}

/// The three `tool_execution_*` events of a nested call reach the extensions with the parent; the
/// loop's own events of the same kinds report none.
#[tokio::test]
async fn tool_execution_events_of_a_nested_call_carry_the_parent_and_the_loops_do_not() {
    let (host, seen) = loaded().await;
    let parent: ToolCallId = "call".into();
    let nested: ToolCallId = "call/1".into();
    let cancel = CancelToken::new();
    let events = [
        NestedToolExecutionEvent::ToolExecutionStart {
            tool_call_id: nested.clone(),
            tool_name: "read".into(),
            args: json!({}),
            parent_tool_call_id: parent.clone(),
        },
        NestedToolExecutionEvent::ToolExecutionUpdate {
            tool_call_id: nested.clone(),
            tool_name: "read".into(),
            args: json!({}),
            partial_result: json!({ "content": [] }),
            parent_tool_call_id: parent.clone(),
        },
        NestedToolExecutionEvent::ToolExecutionEnd {
            duration_ms: None,
            tool_call_id: nested.clone(),
            tool_name: "read".into(),
            result: Value::Null,
            is_error: false,
            parent_tool_call_id: parent.clone(),
        },
    ];
    for event in &events {
        host.emit_nested_tool_execution(event, &cancel).await;
    }
    // The loop's own event, through the ordinary subscriber.
    host.subscriber()
        .on_event(
            &AgentEvent::ToolExecutionEnd {
                duration_ms: None,
                tool_call_id: parent.clone(),
                tool_name: "codemode".into(),
                result: Value::Null,
                is_error: false,
            },
            CancelToken::new(),
        )
        .await;

    let nested_parent = Some("call".to_string());
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            (
                "tool_execution_start",
                "call/1".to_string(),
                nested_parent.clone()
            ),
            (
                "tool_execution_update",
                "call/1".to_string(),
                nested_parent.clone()
            ),
            ("tool_execution_end", "call/1".to_string(), nested_parent),
            ("tool_execution_end", "call".to_string(), None),
        ]
    );
}

/// An extension that does not override the nested entry point is still dispatched the event: the
/// default is the ordinary dispatch, which is what keeps a gate written before nested calls
/// existed covering them.
#[tokio::test]
async fn an_extension_that_ignores_parents_still_receives_nested_events() {
    struct Plain(Arc<Mutex<Vec<String>>>);
    #[async_trait::async_trait]
    impl crate::Extension for Plain {
        fn id(&self) -> &ExtensionId {
            static ID: std::sync::LazyLock<ExtensionId> =
                std::sync::LazyLock::new(|| "plain".into());
            &ID
        }
        fn kind(&self) -> crate::ExtKind {
            crate::ExtKind::Native
        }
        fn subscriptions(&self) -> crate::Subscriptions {
            crate::Subscriptions::empty().with(EventKind::ToolCall)
        }
        async fn invoke_event(
            &self,
            ev: &HostEvent,
            _cancel: &CancelToken,
        ) -> Result<HookOutcome, crate::ExtError> {
            if let HostEvent::ToolCall { call_id, .. } = ev {
                self.0.lock().unwrap().push(call_id.to_string());
            }
            Ok(HookOutcome::Noop)
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = host();
    host.dispatcher()
        .add(Arc::new(Plain(seen.clone())))
        .unwrap();
    let ev = HostEvent::ToolCall {
        call_id: "call/1".into(),
        name: "read".into(),
        input: json!({}),
    };
    host.dispatcher()
        .dispatch_block_mutate_nested(ev, &"call".into(), &CancelToken::new())
        .await;
    assert_eq!(*seen.lock().unwrap(), vec!["call/1".to_string()]);
}

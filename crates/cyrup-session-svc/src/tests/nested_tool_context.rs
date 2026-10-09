//! CODE-006, extension surface — `ExtensionToolContext.executeTool` / `.tools` driven through a
//! REAL session, for the native tier.
//!
//! pi hands every extension tool's `execute` a `ctx` whose `executeTool` is bound to that call
//! (`core/extensions/types.ts:367-395`, `agent-session.ts:3420-3421` @v1.0.1). Here the native
//! extension's tool reads [`ExtensionToolContext::current`] — bound by the registered-tool wrapper
//! the extension host puts around every tool, onto the session's [`cyrup_ext::NestedToolRunner`] —
//! and calls `read`. The WASM tier is the same session seam, driven in
//! `cyrup-it/tests/ext/wasm_nested_tool_calls.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, NestedCallStatus, StopReason, TerminateHint, Tool,
    ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{
    EventKind, ExecuteToolOptions, ExtError, ExtensionToolContext, HookOutcome, HostCtx, HostEvent,
    InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};

/// What the extension tool saw through its context.
#[derive(Default)]
struct Seen {
    /// `(nested call id, is_error, text)`.
    outcome: Option<(String, bool, String)>,
    /// The call id the context was bound to.
    bound_to: Option<String>,
    /// `ctx.tools` names.
    callable: Vec<String>,
}

/// An extension tool that runs `read` on `a.txt` through its context.
struct ReadThroughContext {
    name: &'static str,
    seen: Arc<Mutex<Seen>>,
    params: Value,
}

#[async_trait::async_trait]
impl Tool for ReadThroughContext {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let ctx = ExtensionToolContext::current()
            .ok_or_else(|| ToolError::new("no extension tool context is bound to this call"))?;
        let callable: Vec<String> = ctx.tools().iter().map(|t| t.name().to_string()).collect();
        let outcome = ctx
            .execute_tool(
                "read",
                json!({ "path": "a.txt" }),
                ExecuteToolOptions::default(),
            )
            .await;
        let text: String = outcome
            .result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        let mut seen = self.seen.lock().unwrap();
        seen.bound_to = Some(ctx.call_id().to_string());
        seen.callable = callable;
        seen.outcome = Some((outcome.tool_call.id.to_string(), outcome.is_error, text));
        assert_eq!(ctx.call_id(), &call_id);
        Ok(ToolResult {
            content: vec![Content::text("through the context")],
            ..ToolResult::default()
        })
    }
}

/// `(kind, call id, parent)` of every tool event, as the ctx reports them.
type ExtSeen = Arc<Mutex<Vec<(&'static str, String, Option<String>)>>>;

struct ContextExt {
    tool: Arc<dyn Tool>,
    seen: ExtSeen,
}

#[async_trait::async_trait]
impl NativeExtension for ContextExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("context-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::clone(&self.tool));
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::ToolExecStart,
            EventKind::ToolExecEnd,
        ]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        let (kind, id) = match ev {
            HostEvent::ToolCall { call_id, .. } => ("tool_call", call_id),
            HostEvent::ToolResult { call_id, .. } => ("tool_result", call_id),
            HostEvent::ToolExecStart { call_id, .. } => ("tool_execution_start", call_id),
            HostEvent::ToolExecEnd { call_id, .. } => ("tool_execution_end", call_id),
            _ => return HookOutcome::Noop,
        };
        self.seen.lock().unwrap().push((
            kind,
            id.to_string(),
            ctx.parent_tool_call_id().map(ToString::to_string),
        ));
        let _ = TerminateHint::Unspecified;
        HookOutcome::Noop
    }
}

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
    std::fs::write(cwd.join("a.txt"), "hello from a.txt\n").unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn faux_calling(tool: &'static str) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call(tool.to_string(), json!({}))],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    faux
}

struct Run {
    session: Arc<AgentSession>,
    events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    seen: Arc<Mutex<Seen>>,
    ext_seen: ExtSeen,
}

async fn run_native_tool(fx: &Fixture) -> Run {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let ext_seen: ExtSeen = Arc::new(Mutex::new(Vec::new()));
    let tool: Arc<dyn Tool> = Arc::new(ReadThroughContext {
        name: "ctx_caller",
        seen: Arc::clone(&seen),
        params: json!({ "type": "object", "properties": {}, "additionalProperties": true }),
    });
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux_calling("ctx_caller") as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(ContextExt {
            tool,
            seen: Arc::clone(&ext_seen),
        }))
        .build()
        .await
        .unwrap()
        .into_shared();

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut stream = session.subscribe();
    let sink = Arc::clone(&events);
    tokio::spawn(async move {
        while let Some(ev) = stream.next().await {
            sink.lock().unwrap().push(ev);
        }
    });
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let started = std::time::Instant::now();
    while !events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, AgentSessionEvent::AgentSettled { .. }))
        && started.elapsed() < std::time::Duration::from_secs(10)
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Run {
        session,
        events,
        seen,
        ext_seen,
    }
}

/// A native extension tool calls `read` through `ctx.executeTool`: it gets the file, the call is
/// `<its call id>/1`, the extension's own `tool_call` / `tool_result` / `tool_execution_*` handlers
/// see it with `parent_tool_call_id`, the session's subscribers get the nested events, and the
/// persisted tool-result row carries `nestedCalls` — through `AgentSession::execute_nested_tool`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_extension_tool_calls_read_through_its_context() {
    let fx = fixture();
    let run = run_native_tool(&fx).await;

    // The tool's own view.
    let parent = run
        .events
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match e {
            AgentSessionEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                ..
            } if tool_name == "ctx_caller" => Some(tool_call_id.to_string()),
            _ => None,
        })
        .expect("the model-issued call started");
    {
        let seen = run.seen.lock().unwrap();
        let (id, is_error, text) = seen.outcome.clone().expect("the tool ran to the end");
        assert!(!is_error, "{text}");
        assert!(text.contains("hello from a.txt"), "{text}");
        assert_eq!(id, format!("{parent}/1"));
        assert_eq!(seen.bound_to.as_deref(), Some(parent.as_str()));
        assert!(
            seen.callable.iter().any(|n| n == "read"),
            "ctx.tools lists the callable tools: {:?}",
            seen.callable
        );
        assert_eq!(
            {
                let mut expected = run.session.callable_tool_names();
                expected.sort();
                expected
            },
            {
                let mut got = seen.callable.clone();
                got.sort();
                got
            },
            "ctx.tools is the session's callable set"
        );
    }

    // The extension's handlers: the nested call's events, each with the calling id as its parent;
    // the model-issued call's with none.
    let ext_seen = run.ext_seen.lock().unwrap().clone();
    let nested_id = format!("{parent}/1");
    for kind in [
        "tool_call",
        "tool_result",
        "tool_execution_start",
        "tool_execution_end",
    ] {
        assert!(
            ext_seen.contains(&(kind, nested_id.clone(), Some(parent.clone()))),
            "{kind} of the nested call carries its parent: {ext_seen:?}"
        );
        assert!(
            ext_seen.contains(&(kind, parent.clone(), None)),
            "{kind} of the model-issued call carries none: {ext_seen:?}"
        );
    }

    // The session's subscribers.
    let nested_events = run
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentSessionEvent::NestedToolExecutionStart { parent_tool_call_id, .. }
                    | AgentSessionEvent::NestedToolExecutionEnd { parent_tool_call_id, .. }
                    if parent_tool_call_id.as_str() == parent
            )
        })
        .count();
    assert_eq!(nested_events, 2);

    // The persisted row.
    let row = run
        .session
        .messages()
        .await
        .into_iter()
        .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "ctx_caller"))
        .expect("the tool result was persisted");
    let Message::ToolResult { nested_calls, .. } = &row else {
        panic!("not a tool result");
    };
    let nested = nested_calls.clone().expect("nestedCalls is on the row");
    assert_eq!(nested.calls.len(), 1);
    assert_eq!(nested.calls[0].id.to_string(), nested_id);
    assert_eq!(nested.calls[0].name, "read");
    assert_eq!(nested.calls[0].status, NestedCallStatus::Ok);
    assert_eq!(
        serde_json::to_value(&row).unwrap()["nestedCalls"]["calls"][0]["status"],
        "ok"
    );
}

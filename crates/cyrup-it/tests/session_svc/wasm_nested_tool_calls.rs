//! CODE-006, extension surface, WASM tier — a guest tool's `ctx.execute_tool` through a REAL session.
//!
//! pi hands every extension tool's `execute` a `ctx` whose `executeTool` runs another tool through
//! the session's hooks and records it on the calling tool's result (`core/extensions/types.ts:367-395`,
//! `agent-session.ts:3420-3421` @v1.0.1). The `cyrup-ext-sdk` demo component's `nested_probe` tool
//! calls `ToolCall::execute_tool` / `ToolCall::tools`, which cross the `host-tool.execute-tool` /
//! `callable-tools` imports into `AgentSession::execute_nested_tool`.
//!
//! Two instances of the demo component are loaded. `demo` owns the tool and is executing it, so the
//! nested call's events are not delivered to it (its instance lock is held by the call that makes
//! them); `observer` is a second extension, and it is the one whose guest `tool_call` /
//! `tool_result` / `tool_execution_*` handlers prove `parent-tool-call-id` crosses the WIT boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, NestedCallStatus, StopReason, Tool, ToolCallId,
    ToolError, ToolResult, ToolUpdate, ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::bins;

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

/// The model calls `nested_probe` with `args`, then answers.
fn faux_calling_probe(args: Value) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call("nested_probe".to_string(), args.clone())],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    faux
}

/// A native tool that streams one partial result before it answers — what a guest's
/// `ExecuteToolOptions::on_update` is for.
struct Streamer {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for Streamer {
    fn name(&self) -> &str {
        "streamer"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        mut on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        on_update(ToolUpdate {
            content: vec![Content::text("tick")],
            ..ToolUpdate::default()
        });
        Ok(ToolResult {
            content: vec![Content::text("streamed")],
            ..ToolResult::default()
        })
    }
}

struct StreamerExt;

#[async_trait::async_trait]
impl NativeExtension for StreamerExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("streamer-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(Streamer {
            params: json!({ "type": "object", "properties": {}, "additionalProperties": true }),
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A native tool that waits until it is cancelled — what a deadline or an aborted signal on a
/// guest's `ExecuteToolOptions` has to end.
struct Sleeper {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for Sleeper {
    fn name(&self) -> &str {
        "sleeper"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        cancel.cancelled().await;
        Err(ToolError::new("sleeper was cancelled"))
    }
}

struct SleeperExt;

#[async_trait::async_trait]
impl NativeExtension for SleeperExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("sleeper-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(Sleeper {
            params: json!({ "type": "object", "properties": {}, "additionalProperties": true }),
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

struct Loaded {
    session: Arc<cyrup_session_svc::AgentSession>,
    demo: Arc<cyrup_ext::host::LiveExtension>,
    observer: Arc<cyrup_ext::host::LiveExtension>,
}

async fn session_with_two_guests(fx: &Fixture, provider: Arc<FauxProvider>) -> Loaded {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(provider as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(StreamerExt))
        .with_native_extension(Arc::new(SleeperExt))
        .build()
        .await
        .unwrap()
        .into_shared();
    let bytes = bins::component_bytes();
    let caps = cyrup_ext::Capabilities::host_granted();
    let demo = session
        .load_wasm_extension(ExtensionId::from("demo"), &bytes, &caps)
        .await
        .expect("load the demo guest");
    let observer = session
        .load_wasm_extension(ExtensionId::from("observer"), &bytes, &caps)
        .await
        .expect("load the observing guest");
    session.bind_extensions().await;
    let mut names = session.active_tool_names();
    if !names.iter().any(|n| n == "nested_probe") {
        names.push("nested_probe".to_string());
        session.set_active_tools_by_name(&names).await;
    }
    Loaded {
        session,
        demo,
        observer,
    }
}

fn text_of(m: &Message) -> String {
    match m {
        Message::ToolResult { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

async fn probe_result(session: &cyrup_session_svc::AgentSession) -> Message {
    session
        .messages()
        .await
        .into_iter()
        .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "nested_probe"))
        .expect("the guest tool's result was persisted")
}

/// A guest tool calls `read` through `ToolCall::execute_tool`: the guest gets the file back, the call
/// is `<its call id>/1`, the persisted result row carries `nestedCalls`, and ANOTHER guest's
/// handlers see the nested call's events with the calling id as their `parent-tool-call-id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tool_calls_read_through_the_session_and_another_guest_sees_the_parent() {
    let fx = fixture();
    let loaded = session_with_two_guests(
        &fx,
        faux_calling_probe(json!({ "tool": "read", "args": { "path": "a.txt" } })),
    )
    .await;

    let _ = loaded.session.prompt("go").await.unwrap();
    loaded.session.wait_for_idle().await;

    let row = probe_result(&loaded.session).await;
    let Message::ToolResult {
        tool_call_id,
        nested_calls,
        ..
    } = &row
    else {
        panic!("not a tool result");
    };
    let parent = tool_call_id.to_string();
    let nested_id = format!("{parent}/1");

    // The guest's view of the outcome.
    let text = text_of(&row);
    assert!(
        text.starts_with(&format!("nested {nested_id} error=false")),
        "{text}"
    );
    assert!(text.contains("hello from a.txt"), "{text}");

    // The persisted row.
    let nested = nested_calls.clone().expect("nestedCalls is on the row");
    assert_eq!(nested.calls.len(), 1);
    assert_eq!(nested.calls[0].id.to_string(), nested_id);
    assert_eq!(nested.calls[0].name, "read");
    assert_eq!(nested.calls[0].status, NestedCallStatus::Ok);

    // The OTHER guest: every nested event reached its handlers with the parent.
    let notes = loaded.observer.guest().notifications();
    for expected in [
        format!("demo: tool_call read parent={parent} id={nested_id}"),
        format!("demo: tool_execution_start {nested_id} parent={parent}"),
        format!("demo: tool_execution_end {nested_id} parent={parent}"),
    ] {
        assert!(
            notes.iter().any(|n| n == &expected),
            "the observer never saw {expected:?}: {notes:?}"
        );
    }
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("demo: tool_result read usage=")
                && n.ends_with(&format!(" parent={parent}"))),
        "the observer's tool_result handler (the 18-argument, pointer-passed export) saw the parent: {notes:?}"
    );
    // ...and the model-issued call reached it with none.
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("nested_probe") && n.contains("parent=")),
        "a model-issued call has no parent: {notes:?}"
    );

    // The instance whose tool made the call was not asked (see the module doc).
    assert!(
        !loaded
            .demo
            .guest()
            .notifications()
            .iter()
            .any(|n| n.contains("parent=")),
        "the instance executing the calling tool received nested-call events"
    );
}

/// `ToolCall::tools` — pi `ctx.tools` — is the session's callable set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tool_lists_the_callable_tools() {
    let fx = fixture();
    let loaded = session_with_two_guests(&fx, faux_calling_probe(json!({ "list": true }))).await;

    let _ = loaded.session.prompt("go").await.unwrap();
    loaded.session.wait_for_idle().await;

    let text = text_of(&probe_result(&loaded.session).await);
    let listed: Vec<String> = text
        .strip_prefix("tools: ")
        .unwrap_or_else(|| panic!("{text}"))
        .split(',')
        .map(str::to_string)
        .collect();
    assert!(listed.contains(&"read[direct]".to_string()), "{listed:?}");
    let mut expected: Vec<String> = loaded.session.callable_tool_names().into_iter().collect();
    expected.sort();
    let mut got: Vec<String> = listed
        .iter()
        .map(|t| t.split('[').next().unwrap().to_string())
        .collect();
    got.sort();
    assert_eq!(got, expected, "ctx.tools is the session's callable set");
}

/// A guest tool that calls a tool of its OWN extension is refused by name instead of waiting on
/// the instance lock its own call holds, and the guest carries on with the refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tool_calling_its_own_extensions_tool_is_refused_not_deadlocked() {
    let fx = fixture();
    let loaded = session_with_two_guests(
        &fx,
        faux_calling_probe(json!({ "tool": "signal_probe", "args": {} })),
    )
    .await;

    let _ = loaded.session.prompt("go").await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        loaded.session.wait_for_idle(),
    )
    .await
    .expect("the run settles instead of deadlocking on the instance lock");

    let text = text_of(&probe_result(&loaded.session).await);
    assert!(text.contains("error=true"), "{text}");
    assert!(
        text.contains("a tool cannot call a tool of its own extension"),
        "{text}"
    );
}

/// pi's `options.onUpdate` of `ctx.executeTool`: the partial results the nested tool streamed reach
/// the guest's `on_update` — batched, once the call has settled — and the guest gets the result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tools_on_update_receives_the_nested_tools_partial_results() {
    let fx = fixture();
    let loaded = session_with_two_guests(
        &fx,
        faux_calling_probe(json!({ "tool": "streamer", "args": {}, "collect": true })),
    )
    .await;

    let _ = loaded.session.prompt("go").await.unwrap();
    loaded.session.wait_for_idle().await;

    let text = text_of(&probe_result(&loaded.session).await);
    assert!(
        text.contains("error=false partials=1 :: streamed"),
        "{text}"
    );
}

/// CODE-016, the `signal` gap: a guest that names a signal it already aborted cancels the nested
/// call before it starts. Without the option the sleeper would wait for the calling tool's own
/// cancellation, which never comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_call_is_cancelled_by_a_signal_the_guest_already_aborted() {
    let fx = fixture();
    let loaded = session_with_two_guests(
        &fx,
        faux_calling_probe(json!({
            "tool": "sleeper", "args": {}, "abort_first": "stop", "signal_id": "stop"
        })),
    )
    .await;

    let _ = loaded.session.prompt("go").await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        loaded.session.wait_for_idle(),
    )
    .await
    .expect("the aborted signal ended the nested call instead of waiting for it");

    // The session refuses to start a call whose signal is already aborted, so the sleeper never ran.
    let text = text_of(&probe_result(&loaded.session).await);
    assert!(text.contains("error=true"), "{text}");
    assert!(text.contains("Operation aborted"), "{text}");
}

/// CODE-016, the `signal` gap, the half a suspended guest can still use: a deadline. A signal the
/// guest names but never aborted does not cancel the call; the deadline does, no sooner than asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_call_is_cancelled_when_its_deadline_passes() {
    let fx = fixture();
    let loaded = session_with_two_guests(
        &fx,
        faux_calling_probe(json!({
            "tool": "sleeper", "args": {}, "signal_id": "never-aborted", "timeout_ms": 400
        })),
    )
    .await;

    let started = std::time::Instant::now();
    let _ = loaded.session.prompt("go").await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        loaded.session.wait_for_idle(),
    )
    .await
    .expect("the deadline ended the nested call instead of waiting for it");

    assert!(
        started.elapsed() >= std::time::Duration::from_millis(350),
        "a signal nobody aborted must not cancel the call early: {:?}",
        started.elapsed()
    );
    let text = text_of(&probe_result(&loaded.session).await);
    assert!(text.contains("error=true"), "{text}");
    assert!(text.contains("sleeper was cancelled"), "{text}");
}

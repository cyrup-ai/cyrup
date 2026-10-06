//! A `tool_result` handler replaces the structured content of a call the MODEL issued.
//!
//! pi's `ToolResultEventResult` carries `structuredContent` beside `content`, `details`, `isError`
//! and `usage` (`core/extensions/types.ts:1442-1448` @v1.0.1), and the session returns the folded
//! value as `AfterToolCallResult.structuredContent` (`agent-session.ts:692`), so the result the
//! loop settles on, the `tool_execution_end` event and any programmatic caller see the handler's
//! structured content: replaced when returned, dropped when `content` alone was replaced, kept
//! when the handler touched neither.
//!
//! These run the real loop against the real `PolicyHooks` -> `ExtHooks` -> dispatcher chain. The
//! model never sees structured content, so the observation point is the `tool_execution_end`
//! event's `result`, which is `finalized.result` verbatim (`emitToolExecutionEnd`). The nested-call
//! counterpart, where a script reads it, is in `codemode.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig};
use cyrup_core::{
    CancelToken, Content, ExtensionId, StopReason, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use serde_json::{Value, json};
use tempfile::TempDir;

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

fn stats() -> Value {
    json!({ "files": 2, "names": ["a", "b"] })
}

/// A tool that returns text and structured content, like pi's `bash`.
struct StatsTool {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for StatsTool {
    fn name(&self) -> &str {
        "stats"
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
        Ok(ToolResult {
            content: vec![Content::text("2 files")],
            structured_content: Some(stats()),
            ..Default::default()
        })
    }
}

/// What the handlers do with the result.
#[derive(Clone, Copy)]
enum Act {
    /// Look only.
    Observe,
    /// Replace the content and the structured content.
    ReplaceBoth,
    /// Replace the content alone.
    ReplaceContentOnly,
    /// Replace the structured content alone.
    ReplaceStructuredOnly,
    /// Patch only `details`.
    DetailsOnly,
}

struct Ext {
    act: Act,
}

#[async_trait::async_trait]
impl NativeExtension for Ext {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("structured-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::ToolResult { .. } = ev else {
            return HookOutcome::Noop;
        };
        let (content, structured_content, details) = match self.act {
            Act::Observe => return HookOutcome::Noop,
            Act::ReplaceBoth => (
                Some(vec![Content::text("0 files")]),
                Some(Box::new(json!({ "files": 0, "names": [] }))),
                None,
            ),
            Act::ReplaceContentOnly => (Some(vec![Content::text("redacted")]), None, None),
            Act::ReplaceStructuredOnly => (
                None,
                Some(Box::new(json!({ "files": 9, "names": [] }))),
                None,
            ),
            Act::DetailsOnly => (None, None, Some(json!({ "audited": true }))),
        };
        HookOutcome::Mutate(EventPatch::ToolResult {
            content,
            details,
            structured_content,
            is_error: None,
            usage: None,
            terminate: None,
        })
    }
}

/// Drive one model-issued `stats` call; return the finalized tool result message and the
/// `result` of its `tool_execution_end` event.
async fn run(act: Act) -> (cyrup_agent::ToolResultMessage, Value) {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call("stats".to_string(), json!({}))],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.custom_tools = vec![Arc::new(StatsTool {
        params: json!({ "type": "object" }),
    }) as Arc<dyn Tool>];
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(Ext { act }))
        .build()
        .await
        .unwrap()
        .into_shared();
    let mut names = session.active_tool_names();
    names.push("stats".to_string());
    session.set_active_tools_by_name(&names).await;

    let ends: Arc<Mutex<Vec<Value>>> = Arc::default();
    let mut stream = session.subscribe();
    let sink = Arc::clone(&ends);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        while let Some(event) = stream.next().await {
            if let AgentSessionEvent::ToolExecutionEnd { result, .. } = event {
                sink.lock().unwrap().push(result);
            }
        }
    });

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let started = std::time::Instant::now();
    while ends.lock().unwrap().is_empty() && started.elapsed() < std::time::Duration::from_secs(10)
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let result = session
        .agent_messages()
        .await
        .into_iter()
        .find_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.tool_name == "stats" => Some(t),
            _ => None,
        })
        .expect("a stats result");
    let end = ends.lock().unwrap().first().cloned().expect("an end event");
    (result, end)
}

fn text_of(message: &cyrup_agent::ToolResultMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// Nothing replaced: the tool's own structured content is what the event carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_that_only_observes_leaves_the_structured_content() {
    let (result, end) = run(Act::Observe).await;
    assert_eq!(text_of(&result), "2 files");
    assert_eq!(end["structuredContent"], stats());
}

/// Upstream `keeps structured content that tool_result handlers replace along with the content`,
/// for a call the model issued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_structured_content_that_a_handler_replaces_along_with_the_content() {
    let (result, end) = run(Act::ReplaceBoth).await;
    assert_eq!(text_of(&result), "0 files");
    assert_eq!(end["structuredContent"], json!({ "files": 0, "names": [] }));
}

/// Replacing `content` without returning `structuredContent` drops it
/// (`ToolResultEventResult`, `types.ts:1436-1441`): the key is absent from the settled result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacing_the_content_alone_drops_the_structured_content() {
    let (result, end) = run(Act::ReplaceContentOnly).await;
    assert_eq!(text_of(&result), "redacted");
    assert!(
        end.as_object().unwrap().get("structuredContent").is_none(),
        "dropped, not null: {end}"
    );
}

/// A handler that returns only `structuredContent` replaces it and leaves the text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_replacing_only_the_structured_content_leaves_the_text() {
    let (result, end) = run(Act::ReplaceStructuredOnly).await;
    assert_eq!(text_of(&result), "2 files");
    assert_eq!(end["structuredContent"], json!({ "files": 9, "names": [] }));
}

/// `details` alone is not a content replacement, so the structured content stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_details_only_handler_keeps_the_structured_content() {
    let (result, end) = run(Act::DetailsOnly).await;
    assert_eq!(text_of(&result), "2 files");
    assert_eq!(result.details, Some(json!({ "audited": true })));
    assert_eq!(end["structuredContent"], stats());
}

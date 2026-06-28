//! Full-stack integration tests for the `AgentSession` facade (arch-11 §11).
//!
//! These drive the WHOLE wired stack via the scripted `FauxProvider` (no network/tokens): the
//! provider streams an assistant message carrying a tool call, the tool executes through the
//! registry, the result feeds back, and a final assistant message arrives — while a NATIVE built-in
//! extension observes the tool call through the wired ext seams and the session tree is persisted to
//! disk across the turn.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_agent::AgentMessage;
use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink,
};
use cyrup_ext::{EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSessionEvent, InputSource, SessionBuilder, SessionConfig, SessionServiceError, UserInput,
};
use futures::StreamExt;
use tempfile::TempDir;

// ----------------------------------------------------------------------------------------------
// A native built-in extension that observes (and records) tool_call / tool_result events.
// ----------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct ToolObserver {
    seen_calls: Arc<Mutex<Vec<String>>>,
    seen_results: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for ToolObserver {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("tool-observer")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolCall, EventKind::ToolResult]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ToolCall { name, .. } => {
                self.seen_calls.lock().unwrap().push(name.clone());
            }
            HostEvent::ToolResult { name, .. } => {
                self.seen_results.lock().unwrap().push(name.clone());
            }
            _ => {}
        }
        HookOutcome::Noop
    }
}

// ----------------------------------------------------------------------------------------------
// A tool that blocks until its cancel token fires — to exercise cancellation end-to-end.
// ----------------------------------------------------------------------------------------------

struct SleeperTool {
    params: serde_json::Value,
    cancelled: Arc<Mutex<bool>>,
}

impl SleeperTool {
    fn new(cancelled: Arc<Mutex<bool>>) -> Self {
        Self { params: serde_json::json!({"type": "object", "properties": {}}), cancelled }
    }
}

#[async_trait::async_trait]
impl Tool for SleeperTool {
    fn name(&self) -> &str {
        "sleeper"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        cancel.cancelled().await;
        *self.cancelled.lock().unwrap() = true;
        Ok(ToolResult { content: vec![Content::text("aborted")], details: None, terminate: false })
    }
}

struct SleeperExt {
    cancelled: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl NativeExtension for SleeperExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("sleeper-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(SleeperTool::new(self.cancelled.clone())));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

// ----------------------------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------------------------

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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true); // --approve: deterministic trusted project
    cfg
}

fn write_agents_md(fx: &Fixture, marker: &str) {
    std::fs::write(fx.cwd.join("AGENTS.md"), format!("# Project\n{marker}\n")).unwrap();
}

fn write_skill(fx: &Fixture, name: &str, desc: &str) {
    let dir = fx.agent_dir.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {desc}\n---\n\nBody.\n"),
    )
    .unwrap();
}

// ----------------------------------------------------------------------------------------------
// The TRUE end-to-end test (R-11-023, arch-11 §11).
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn end_to_end_tool_call_round_trip_with_native_extension() {
    let fx = fixture();
    write_agents_md(&fx, "PROJECT_CONTEXT_MARKER");
    write_skill(&fx, "demoskill", "use this when you need a demo");

    // Scripted provider: turn 1 = assistant w/ a `write` tool call; turn 2 = final assistant text.
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("write", serde_json::json!({"path": "hello.txt", "content": "hi"}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("all done")], StopReason::Stop),
    ]);

    let observer = ToolObserver::default();
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(observer.clone()))
        .build()
        .await
        .expect("build session");

    // (c) the assembled system prompt includes tools / skills / context sections.
    let prompt = session.system_prompt().to_string();
    assert!(prompt.contains("Available tools:"), "tools section missing:\n{prompt}");
    assert!(prompt.contains("write"), "write tool snippet missing");
    assert!(prompt.contains("PROJECT_CONTEXT_MARKER"), "context file not injected:\n{prompt}");
    assert!(prompt.contains("<project_instructions"), "project_instructions wrapper missing");
    assert!(prompt.contains("Available skills"), "skills section missing:\n{prompt}");
    assert!(prompt.contains("demoskill"), "skill pointer missing");

    // Drive a prompt and collect the full event stream.
    let stream = session
        .prompt(UserInput::text("please write the file", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    // (a) the AgentSessionEvent stream order is correct.
    let kinds: Vec<&str> = events.iter().map(AgentSessionEvent::kind).collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "stream must start with agent_start: {kinds:?}");
    assert_eq!(kinds.last(), Some(&"agent_end"), "stream must end with agent_end: {kinds:?}");
    let tes = kinds.iter().position(|k| *k == "tool_execution_start").expect("tool_execution_start");
    let tee = kinds.iter().position(|k| *k == "tool_execution_end").expect("tool_execution_end");
    assert!(tes < tee, "tool exec start must precede end");

    // message_end role order = user -> assistant(toolCall) -> toolResult -> assistant.
    let roles: Vec<&'static str> = events
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::MessageEnd { message } => Some(role_with_toolcall(message)),
            _ => None,
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant+toolcall", "toolResult", "assistant"],
        "message_end roles out of order: {roles:?}"
    );

    // (d) the native extension observed the tool call through the wired seams.
    assert_eq!(
        observer.seen_calls.lock().unwrap().clone(),
        vec!["write".to_string()],
        "extension did not observe the tool_call via ExtHooks/dispatcher"
    );
    assert_eq!(
        observer.seen_results.lock().unwrap().clone(),
        vec!["write".to_string()],
        "extension did not observe the tool_result"
    );

    // (b) the session tree on disk contains user -> assistant(toolCall) -> toolResult -> assistant.
    let msgs = session.messages().await;
    assert_eq!(msgs.len(), 4, "expected 4 persisted messages, got {}", msgs.len());
    assert!(matches!(msgs[0], Message::User { .. }), "msg0 should be user");
    match &msgs[1] {
        Message::Assistant(a) => assert!(
            a.content.iter().any(|c| matches!(c, Content::ToolCall(_))),
            "assistant message must carry the tool call"
        ),
        other => panic!("msg1 should be assistant, got {other:?}"),
    }
    assert!(matches!(msgs[2], Message::ToolResult { .. }), "msg2 should be toolResult");
    assert!(matches!(msgs[3], Message::Assistant(_)), "msg3 should be assistant");

    // The tree is durable on disk (not just in memory).
    let file = session.session_file().await.expect("persisted session file");
    let on_disk = std::fs::read_to_string(&file).expect("read session file");
    assert!(on_disk.contains("\"role\":\"toolResult\""), "tool result not persisted:\n{on_disk}");
    assert!(on_disk.contains("hello.txt"), "tool call args not persisted");

    // The tool actually executed against the workspace through the registry.
    assert!(fx.cwd.join("hello.txt").exists(), "write tool did not create the file");
    assert_eq!(std::fs::read_to_string(fx.cwd.join("hello.txt")).unwrap(), "hi");
}

fn role_with_toolcall(m: &AgentMessage) -> &'static str {
    match m {
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant(a) => {
            if a.content.iter().any(|c| matches!(c, Content::ToolCall(_))) {
                "assistant+toolcall"
            } else {
                "assistant"
            }
        }
        AgentMessage::ToolResult(_) => "toolResult",
        AgentMessage::Custom { .. } => "custom",
    }
}

// ----------------------------------------------------------------------------------------------
// Focused unit tests: model resolution, trust-gated context, cancellation.
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn model_resolution_wiring() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    // Explicit pattern resolves to the catalog model + wires it into the agent.
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    let session = SessionBuilder::new(faux.clone(), cfg).build().await.unwrap();
    let m = session.model();
    assert_eq!(m.model.as_str(), "faux-1");
    assert_eq!(m.provider.as_str(), "faux");

    // An unknown pattern is a typed ModelNotFound error (no panic).
    let mut cfg2 = base_config(&fx);
    cfg2.model_pattern = Some("does-not-exist".to_string());
    match SessionBuilder::new(faux, cfg2).build().await {
        Err(SessionServiceError::ModelNotFound(_)) => {}
        Err(other) => panic!("expected ModelNotFound, got {other:?}"),
        Ok(_) => panic!("expected ModelNotFound, got Ok"),
    }
}

#[tokio::test]
async fn trust_gated_context_files() {
    let fx = fixture();
    write_agents_md(&fx, "TRUST_GATED_MARKER");
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    // Untrusted (--no-approve): project context files are NOT loaded (R-06-009).
    let mut untrusted = base_config(&fx);
    untrusted.trust_override = Some(false);
    let s_untrusted = SessionBuilder::new(faux.clone(), untrusted).build().await.unwrap();
    assert!(
        !s_untrusted.system_prompt().contains("TRUST_GATED_MARKER"),
        "untrusted session must not inject project context"
    );
    assert!(!s_untrusted.services().project_trusted);

    // Trusted (--approve): the project AGENTS.md is injected.
    let mut trusted = base_config(&fx);
    trusted.trust_override = Some(true);
    let s_trusted = SessionBuilder::new(faux, trusted).build().await.unwrap();
    assert!(
        s_trusted.system_prompt().contains("TRUST_GATED_MARKER"),
        "trusted session must inject project context"
    );
    assert!(s_trusted.services().project_trusted);
}

#[tokio::test]
async fn cancellation_unblocks_a_running_tool() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Turn 1: call the (blocking) sleeper tool. After abort, no more responses are needed.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_tool_call("sleeper", serde_json::json!({}))],
        StopReason::ToolUse,
    )]);

    let cancelled = Arc::new(Mutex::new(false));
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(SleeperExt { cancelled: cancelled.clone() }))
        .build()
        .await
        .unwrap();

    let mut stream = session.prompt("run the sleeper").await.unwrap();

    // Wait until the tool starts executing, then abort the run.
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(ev)) if ev.kind() == "tool_execution_start" => {
                session.abort();
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => panic!("stream ended before the tool started"),
            Err(_) => panic!("timed out waiting for tool_execution_start"),
        }
    }

    // The run must settle (no deadlock) and the tool must have observed cancellation.
    tokio::time::timeout(Duration::from_secs(5), session.wait_for_idle())
        .await
        .expect("wait_for_idle must complete after abort");
    assert!(!session.is_streaming().await, "session must be idle after abort");
    assert!(*cancelled.lock().unwrap(), "sleeper tool did not observe cancellation");
}

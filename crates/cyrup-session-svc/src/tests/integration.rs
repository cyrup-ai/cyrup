//! Full-stack integration tests for the `AgentSession` facade (arch-11 §11): the three proofs that
//! need the WHOLE stack standing up at once, rather than one seam of it. Anything provable against
//! a single seam lives in that seam's own module.
//!
//! These drive the whole wired stack via the scripted `FauxProvider` (no network/tokens): the
//! provider streams an assistant message carrying a tool call, the tool executes through the
//! registry, the result feeds back, and a final assistant message arrives — while a NATIVE built-in
//! extension observes the tool call through the wired ext seams and the session tree is persisted to
//! disk across the turn.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_agent::AgentMessage;
use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink,
};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, fixture, Fixture};
use crate::{AgentSessionEvent, InputSource, SessionBuilder, UserInput};
use futures::StreamExt;

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
        Ok(ToolResult { content: vec![Content::text("aborted")], details: None, terminate: false, ..Default::default() })
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
    // The skills block is pi's `formatSkillsForPrompt` (`packages/coding-agent/src/core/skills.ts:
    // 342-358` @v0.83.0): three lead-in lines, then an `<available_skills>` element holding one
    // `<skill>` per visible skill with `<name>`/`<description>`/`<location>` children. There is no
    // "Available skills" prose heading anywhere in pi — asserting one pinned a format pi does not
    // emit, and it only passed while cyrup's skills section was still unported.
    assert!(
        prompt.contains("The following skills provide specialized instructions for specific tasks."),
        "skills lead-in missing:\n{prompt}"
    );
    assert!(prompt.contains("<available_skills>"), "skills section missing:\n{prompt}");
    assert!(prompt.contains("</available_skills>"), "skills section unclosed:\n{prompt}");
    // The skill's three children, not merely its name appearing somewhere in the prompt.
    assert!(prompt.contains("<name>demoskill</name>"), "skill name missing:\n{prompt}");
    assert!(
        prompt.contains("<description>use this when you need a demo</description>"),
        "skill description missing:\n{prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "<location>{}</location>",
            fx.agent_dir.join("skills").join("demoskill").join("SKILL.md").display()
        )),
        "skill location must be the absolute SKILL.md path pi points the model at:\n{prompt}"
    );

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
    // SEAM-005: the run-scoped stream now closes on `agent_settled`, the event that says the WHOLE
    // run (including any auto-retry / post-run compaction / queued continuation) is done — Pi's
    // `_emitAgentSettled` likewise runs after the post-run loop, in `_runAgentPrompt`'s `finally`
    // (agent-session.ts:1063-1072). `agent_end` is now the second-to-last event, not the last.
    assert_eq!(
        kinds.last(),
        Some(&"agent_settled"),
        "stream must end with agent_settled: {kinds:?}"
    );
    assert_eq!(
        kinds.iter().rev().nth(1),
        Some(&"agent_end"),
        "…immediately preceded by the run's last agent_end: {kinds:?}"
    );
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
        // SESS-043 — the pi role tag is carried on the variant, so this reports what pi's
        // `message.role` would.
        AgentMessage::App { role, .. } => match role.as_str() {
            "bashExecution" => "bashExecution",
            "branchSummary" => "branchSummary",
            "compactionSummary" => "compactionSummary",
            _ => "app",
        },
    }
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

// ----------------------------------------------------------------------------------------------
// An extension that rewrites the system prompt and injects a message at before_agent_start.
// ----------------------------------------------------------------------------------------------

struct PromptRewriter;

#[async_trait::async_trait]
impl NativeExtension for PromptRewriter {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("prompt-rewriter")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::BeforeAgentStart { .. } = ev {
            let inject =
                Message::User { content: vec![Content::text("INJECTED_CONTEXT")], timestamp: 0 };
            HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                system: Some("REWRITTEN_SYSTEM_PROMPT".to_string()),
                inject: Some(Box::new(inject)),
            })
        } else {
            HookOutcome::Noop
        }
    }
}

/// R-06-014 / gap #3: the assembled prompt is now actually offered to `before_agent_start`, and a
/// handler may BOTH replace the system prompt AND inject a message into the run.
#[tokio::test]
async fn before_agent_start_hook_is_invoked_and_applied() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);

    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(PromptRewriter))
        .build()
        .await
        .expect("build");

    // Before any run the agent uses the assembled base prompt.
    let base = session.system_prompt().to_string();
    assert_ne!(base, "REWRITTEN_SYSTEM_PROMPT");

    let stream = session.prompt("hello").await.expect("prompt accepted");
    session.wait_for_idle().await;
    let _ = stream.collect::<Vec<_>>().await;

    // The handler's system-prompt replacement reached the agent (Pi agent-session.ts:1127).
    assert_eq!(session.current_system_prompt().await, "REWRITTEN_SYSTEM_PROMPT");

    // The injected message is part of the persisted run alongside the user prompt.
    let texts: Vec<String> = session
        .messages()
        .await
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "hello"), "original prompt missing: {texts:?}");
    assert!(texts.iter().any(|t| t == "INJECTED_CONTEXT"), "injected message missing: {texts:?}");
}

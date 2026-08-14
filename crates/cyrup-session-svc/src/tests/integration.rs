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
    CancelToken, Content, EntryId, ExtensionId, Message, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink,
};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use crate::{
    AgentSessionEvent, AgentSessionRuntime, ForkPosition, InputSource, SessionBuilder,
    SessionCommand, SessionCommandOutput, SessionConfig, SessionFactory, SessionServiceError,
    SessionTarget, UserInput,
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
    let m = session.model().expect("session must have a resolved model");
    assert_eq!(m.model.as_str(), "faux-1");
    assert_eq!(m.provider.as_str(), "faux");

    // An unknown *bare* pattern (no provider prefix, no explicit --provider) stays a typed
    // ModelNotFound error (Pi `resolveCliModel` only builds a fallback when a provider is known).
    let mut cfg2 = base_config(&fx);
    cfg2.model_pattern = Some("does-not-exist".to_string());
    match SessionBuilder::new(faux.clone(), cfg2).build().await {
        Err(SessionServiceError::ModelNotFound(_)) => {}
        Err(other) => panic!("expected ModelNotFound, got {other:?}"),
        Ok(_) => panic!("expected ModelNotFound, got Ok"),
    }
}

#[tokio::test]
async fn unresolvable_model_on_known_provider_builds_a_custom_fallback() {
    // Pi `buildFallbackModel` (model-resolver.ts:475-501): an unresolvable `--model` id on a KNOWN
    // provider builds a custom-id model and proceeds (no error). The provider is "known" via a
    // `provider/` prefix OR an explicit `--provider`.
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    // `faux/custom-9000`: the `faux/` prefix names the resolved provider → custom fallback.
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux/custom-9000".to_string());
    let session = SessionBuilder::new(faux.clone(), cfg).build().await.unwrap();
    let m = session.model().expect("session must have a resolved model");
    assert_eq!(m.model.as_str(), "custom-9000");
    assert_eq!(m.provider.as_str(), "faux");

    // Bare id + explicit `--provider` (cli_provider_explicit) → custom fallback too.
    let mut cfg2 = base_config(&fx);
    cfg2.model_pattern = Some("totally-made-up".to_string());
    cfg2.cli_provider_explicit = true;
    let session2 = SessionBuilder::new(faux, cfg2).build().await.unwrap();
    assert_eq!(session2.model().expect("session must have a resolved model").model.as_str(), "totally-made-up");
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

/// gap #30 + #12: the queue mirrors + `queue_update` emission, exercised through the `SessionCommand`
/// one-seam surface.
#[tokio::test]
async fn queue_introspection_and_command_seam() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session =
        SessionBuilder::new(faux, base_config(&fx)).build().await.expect("build");

    // Observe queue_update events on a persistent subscription.
    let mut sub = session.subscribe();

    // Route everything through the command seam (arch-11 §2.1).
    let out = session
        .execute(SessionCommand::Steer(UserInput::text("steer-1", InputSource::Rpc)))
        .await
        .expect("steer");
    assert!(matches!(out, SessionCommandOutput::Accepted(_)));
    session
        .execute(SessionCommand::FollowUp(UserInput::text("follow-1", InputSource::Rpc)))
        .await
        .expect("follow_up");

    assert_eq!(session.steering_messages(), vec!["steer-1".to_string()]);
    assert_eq!(session.follow_up_messages(), vec!["follow-1".to_string()]);
    assert_eq!(session.pending_message_count(), 2);

    // A queue_update was emitted (at least one).
    let mut saw_queue_update = false;
    for _ in 0..4 {
        match tokio::time::timeout(Duration::from_millis(200), sub.next()).await {
            Ok(Some(ev)) => {
                if ev.kind() == "queue_update" {
                    saw_queue_update = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(saw_queue_update, "expected a queue_update event");

    // Clear via the seam.
    session.execute(SessionCommand::ClearQueue).await.expect("clear");
    assert_eq!(session.pending_message_count(), 0);

    // The state view reflects the cleared queue.
    let state = session.state_view().await;
    assert_eq!(state.pending_message_count, 0);
    assert_eq!(state.provider.as_deref(), Some("faux"));
}

/// gap #1-11 / R-11-020/021: the AgentSessionRuntime multi-session tier — `new_session` tears down,
/// rebuilds a fresh session, bumps the generation watch, and INVALIDATES prior subscriptions.
#[tokio::test]
async fn runtime_new_session_invalidates_subscriptions_and_bumps_generation() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);

    let provider: Arc<dyn Provider> = faux.clone();
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime =
        AgentSessionRuntime::create(factory, SessionTarget::New).await.expect("runtime");

    assert_eq!(runtime.generation().await, 0);
    let mut gen_watch = runtime.watch_generation();

    // A persistent subscription on the FIRST session.
    let first = runtime.session().await;
    let first_id = first.session_id().clone();
    let mut sub = first.subscribe();
    // Drive one prompt so the first session has content.
    let _stream = first.prompt("first").await.expect("prompt");
    first.wait_for_idle().await;
    drop(first);

    // Replace the session.
    let result = runtime.new_session().await.expect("new_session");
    assert!(!result.cancelled);
    assert_eq!(runtime.generation().await, 1, "generation must bump on replacement");
    assert!(gen_watch.changed().await.is_ok(), "generation watch must fire");
    assert_eq!(*gen_watch.borrow(), 1);

    // The OLD subscription terminates with a SessionReplaced terminal (R-11-021).
    let mut saw_replaced = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(2), sub.next()).await {
            Ok(Some(ev)) => {
                if let AgentSessionEvent::SessionReplaced { generation } = ev {
                    assert_eq!(generation, 1);
                    saw_replaced = true;
                }
            }
            Ok(None) => break, // stream closed after invalidation — expected
            Err(_) => panic!("old subscription did not terminate after replacement"),
        }
    }
    assert!(saw_replaced, "old subscription must receive the SessionReplaced terminal");

    // The new session is fresh (different id, empty transcript).
    let second = runtime.session().await;
    assert_ne!(second.session_id(), &first_id, "new_session must create a distinct session");
    assert!(second.messages().await.is_empty(), "new session must start empty");
}

/// gap #4/#6/#33: entry-anchored fork via the runtime + `getUserMessagesForForking`, plus stats.
#[tokio::test]
async fn runtime_fork_at_entry_and_fork_anchors() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a1")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("a2")], StopReason::Stop),
    ]);

    let provider: Arc<dyn Provider> = faux.clone();
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime =
        AgentSessionRuntime::create(factory, SessionTarget::New).await.expect("runtime");

    let session = runtime.session().await;
    let _stream = session.prompt("the first user message").await.expect("prompt");
    session.wait_for_idle().await;

    // Stats reflect the round-trip.
    let stats = session.session_stats().await;
    assert_eq!(stats.user_messages, 1);
    assert_eq!(stats.assistant_messages, 1);

    // The fork anchors enumerate the user message(s) on the branch.
    let anchors = session.user_messages_for_forking().await;
    assert_eq!(anchors.len(), 1, "one user message anchor");
    assert_eq!(anchors[0].text, "the first user message");
    let anchor_id: EntryId = anchors[0].entry_id.clone();
    drop(session);

    // Fork AT that entry through the runtime → a fresh branched session, generation bumps.
    let fork = runtime.fork(anchor_id, ForkPosition::At).await.expect("fork");
    assert!(!fork.cancelled);
    assert_eq!(runtime.generation().await, 1, "fork replaces the session");
}

// ----------------------------------------------------------------------------------------------
// L6↔L5 additive data seams the TUI `/trust`, `/settings`, and `/resume` selectors source from
// (round 7): trust options + write, settings persist, session list.
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn trust_settings_and_session_list_seams() {
    use crate::{SettingsScope, TrustDecision};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build");

    // ---- /trust: options + write + saved-decision readback ----
    let options = session.project_trust_options();
    assert!(options.iter().any(|o| o.label == "Trust" && o.trusted));
    assert!(options.iter().any(|o| o.label == "Do not trust" && !o.trusted));
    assert_eq!(session.saved_trust_decision(), None, "no decision persisted yet");

    // Persist the "Trust" option's store updates → writes agent_dir/trust.json.
    let trust_opt = options.iter().find(|o| o.label == "Trust").expect("trust option");
    session.write_project_trust(&trust_opt.updates).expect("write trust");
    assert!(session.trust_store_path().exists(), "trust.json written");
    let saved = session.saved_trust_decision().expect("decision now persisted");
    assert!(saved.decision.is_trusted(), "persisted decision is trusted");

    // Round-trip an explicit untrusted decision.
    session
        .write_project_trust(&[(fx.cwd.clone(), Some(TrustDecision::Untrusted))])
        .expect("write untrusted");
    assert!(!session.saved_trust_decision().expect("decision").decision.is_trusted());

    // ---- /settings: persist via the `&self` write seam (the default builder store is in-memory,
    // so this verifies the seam round-trips without error, including the project trust gate). ----
    session
        .persist_setting(SettingsScope::Global, "terminal.showImages", serde_json::json!(false))
        .expect("persist global setting");
    session
        .persist_setting(SettingsScope::Project, "quietStartup", serde_json::json!(true))
        .expect("persist project setting (trusted)");

    // ---- /resume: the session list includes this session (after a turn flushes it to disk) ----
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let _stream = session.prompt("hello world").await.expect("prompt");
    session.wait_for_idle().await;
    let sessions = session.list_sessions();
    assert!(
        sessions.iter().any(|s| s.id.to_string() == session.session_id().to_string()),
        "current session appears in the resume list ({} found)",
        sessions.len()
    );
}

// ---- extension flag threading (Pi resourceLoaderOptions additionalExtensionPaths/noExtensions,
// main.ts:660,664). The `--extension`/`--no-extensions` flags must reach the discovery roots. ----

#[test]
fn extension_discovery_roots_honor_no_extensions_and_explicit_paths() {
    use crate::extension_discovery_roots;

    // Default: project + global roots scanned, no configured paths.
    let mut cfg = SessionConfig::new(PathBuf::from("/work"), PathBuf::from("/agent"));
    let roots = extension_discovery_roots(&cfg);
    assert_eq!(roots.project_cwd, Some(PathBuf::from("/work")));
    assert_eq!(roots.agent_dir, Some(PathBuf::from("/agent")));
    assert!(roots.configured.is_empty());

    // Explicit `--extension` paths become pre-trust configured roots (always loaded).
    cfg.extra_extension_paths = vec![PathBuf::from("/work/ext-a"), PathBuf::from("/work/ext-b")];
    let roots = extension_discovery_roots(&cfg);
    assert_eq!(
        roots.configured,
        vec![PathBuf::from("/work/ext-a"), PathBuf::from("/work/ext-b")]
    );
    // Still discovering project + global.
    assert!(roots.project_cwd.is_some() && roots.agent_dir.is_some());

    // `--no-extensions` disables project + global *discovery*, but explicit `-e` paths still load.
    cfg.no_extensions = true;
    let roots = extension_discovery_roots(&cfg);
    assert_eq!(roots.project_cwd, None);
    assert_eq!(roots.agent_dir, None);
    assert_eq!(
        roots.configured,
        vec![PathBuf::from("/work/ext-a"), PathBuf::from("/work/ext-b")]
    );
}

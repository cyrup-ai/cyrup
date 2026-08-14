//! Round-4 facade/runtime parity tests (vs Pi `agent-session.ts` + `agent-session-runtime.ts`):
//! the runtime `diagnostics` getter, the `newSession`/`switchSession` option bags
//! (`parentSession`/`cwdOverride`), the runtime `reload` op, the in-`prompt` `streamingBehavior`
//! routing (expand-then-queue), and the `input` extension event short-circuit.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{Content, ExtensionId, StopReason, Tool, ToolError, ToolResult, ToolUpdateSink};
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxConfig, FauxModelDefinition, FauxProvider,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSessionRuntime, NewSessionOptions, PromptAccepted, PromptOptions, SessionBuilder,
    SessionConfig, SessionFactory, SessionTarget, StreamingBehavior, SwitchSessionOptions,
};
use tempfile::TempDir;
use tokio::sync::Notify;

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
    cfg.trust_override = Some(true);
    cfg
}

// ============================================================================ #24 diagnostics ====

/// gap #24: the runtime `diagnostics` getter — empty on a clean build, and STILL empty when a
/// resumed session's saved model is gone from the catalog, because pi keeps `modelFallbackMessage`
/// out of `diagnostics`: it is a separate constructor argument
/// (`new AgentSessionRuntime(session, services, createRuntime, result.diagnostics,
/// result.modelFallbackMessage)`, agent-session-runtime.ts:425-431) whose only reader is the
/// interactive `showWarning` (interactive-mode.ts:883-884). `reportDiagnostics` (main.ts:842) must
/// not echo it, or every non-interactive run prints the banner twice.
#[tokio::test]
async fn model_restore_fallback_is_carried_beside_diagnostics_not_inside_them() {
    let fx = fixture();

    // Session 1 persists a `model_change` for `faux/faux-1` (a driven turn flushes the file).
    let faux1 = Arc::new(FauxProvider::new());
    faux1.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let p1: Arc<dyn Provider> = faux1.clone();
    let s1 = SessionBuilder::new(p1.clone(), base_config(&fx)).build().await.unwrap();
    let session_file = s1.session_file().await.expect("session 1 persisted");
    let _ = s1.prompt("hi").await.unwrap();
    s1.wait_for_idle().await;
    drop(s1);

    // A clean runtime (fresh New session) has no diagnostics.
    let clean_factory = Arc::new(SessionFactory::new(p1, base_config(&fx)));
    let clean = AgentSessionRuntime::create(clean_factory, SessionTarget::New).await.unwrap();
    assert!(clean.diagnostics().await.is_empty(), "a clean build has no diagnostics");

    // Resume session 1 with a provider whose catalog LACKS `faux-1` → model restore fails →
    // a `modelFallbackMessage` is produced and surfaced as a runtime diagnostic.
    let other = FauxConfig { models: vec![FauxModelDefinition::new("other-1")], ..FauxConfig::default() };
    let p2: Arc<dyn Provider> = Arc::new(FauxProvider::with_config(other));
    let factory2 = Arc::new(SessionFactory::new(p2, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory2, SessionTarget::Resume(session_file))
        .await
        .unwrap();

    // The fallback IS produced and IS reachable — on its own getter, exactly as pi carries it.
    let fallback = runtime
        .model_fallback_message()
        .await
        .expect("an unrestorable saved model produces a modelFallbackMessage");
    assert!(fallback.contains("faux-1"), "message names the unrestorable model: {fallback}");
    // …and it is NOT duplicated into the diagnostics array that `reportDiagnostics` prints.
    let diags = runtime.diagnostics().await;
    assert!(
        diags.is_empty(),
        "pi's `services.diagnostics` carries no model entry; got {diags:?}"
    );
}

// ============================================================================ #26 option bags ====

/// gap #26: `newSession({parentSession})` records the parent file on the freshly-created session.
#[tokio::test]
async fn new_session_with_records_parent_session() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(SessionFactory::new(faux, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();

    let parent_file = runtime.session().await.session_file().await.expect("persisted").display().to_string();

    let result = runtime
        .new_session_with(NewSessionOptions { parent_session: Some(parent_file.clone()) })
        .await
        .unwrap();
    assert!(!result.cancelled);

    // The new session's JSONL header carries `parentSession`.
    let session = runtime.session().await;
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl text");
    let header_line = jsonl.lines().next().expect("header line");
    let header: serde_json::Value = serde_json::from_str(header_line).unwrap();
    assert_eq!(
        header["parentSession"].as_str(),
        Some(parent_file.as_str()),
        "the new session records its parent file"
    );
}

/// gap #26: `switchSession({cwdOverride})` rebinds the resumed session's cwd-bound services to the
/// caller-supplied cwd instead of deriving it from the session file.
#[tokio::test]
async fn switch_session_with_cwd_override_rebinds_services_cwd() {
    let fx = fixture();
    // A second, existing cwd to rebind onto.
    let cwd2 = fx._tmp.path().join("project2");
    std::fs::create_dir_all(&cwd2).unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();
    let session_file = {
        // Drive a turn so the session file flushes to disk before we re-open it.
        let s = runtime.session().await;
        let file = s.session_file().await.expect("persisted");
        let _ = s.prompt("hi").await.unwrap();
        s.wait_for_idle().await;
        file
    };

    let result = runtime
        .switch_session_with(
            session_file,
            SwitchSessionOptions { cwd_override: Some(cwd2.clone()) },
        )
        .await
        .unwrap();
    assert!(!result.cancelled);

    let session = runtime.session().await;
    assert_eq!(session.services().cwd, cwd2, "cwd_override rebinds the services cwd");
}

/// gap #26: a missing override cwd is rejected at the pre-flight before any teardown.
#[tokio::test]
async fn switch_session_with_missing_override_cwd_is_rejected() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(SessionFactory::new(faux, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();
    let session_file = runtime.session().await.session_file().await.expect("persisted");
    let gen_before = runtime.generation().await;

    let missing = fx._tmp.path().join("does-not-exist");
    let err = runtime
        .switch_session_with(session_file, SwitchSessionOptions { cwd_override: Some(missing) })
        .await
        .unwrap_err();
    assert!(matches!(err, cyrup_session_svc::SessionServiceError::MissingSessionCwd(_)));
    assert_eq!(runtime.generation().await, gen_before, "a rejected switch leaves the session intact");
}

// ================================================================================== #18b reload ====

/// gap #18b: the runtime `reload` op rebuilds the active (persisted) session — preserving its
/// transcript — bumps the generation, runs the `before_start` hook, and re-emits `session_start`.
#[tokio::test]
async fn reload_rebuilds_session_preserving_transcript_and_runs_hook() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("hi there")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();

    // Drive one turn so the persisted session has a transcript.
    {
        let s = runtime.session().await;
        let _ = s.prompt("remember me").await.unwrap();
        s.wait_for_idle().await;
        assert_eq!(s.messages().await.len(), 2, "user + assistant persisted before reload");
    }
    assert_eq!(runtime.generation().await, 0);

    // Reload: the before_start hook fires before session_start; the generation bumps.
    let fired = Arc::new(AtomicBool::new(false));
    let f = fired.clone();
    runtime
        .reload(Some(Box::new(move || f.store(true, Ordering::SeqCst))))
        .await
        .unwrap();

    assert!(fired.load(Ordering::SeqCst), "before_start hook must run on reload");
    assert_eq!(runtime.generation().await, 1, "reload bumps the replacement generation");

    // The rebuilt session re-opened the SAME persisted file, preserving the transcript.
    let reloaded = runtime.session().await;
    assert_eq!(reloaded.messages().await.len(), 2, "reload preserves the persisted transcript");
}

// ============================================================ #13 in-prompt streamingBehavior ====

/// A native extension contributing a `block` tool whose execution parks on a gate, so the test can
/// hold the agent in a streaming state deterministically.
struct BlockTool {
    gate: Arc<Notify>,
    params: serde_json::Value,
}
#[async_trait::async_trait]
impl Tool for BlockTool {
    fn name(&self) -> &str {
        "block"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "Parks until released"
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.gate.notified().await;
        Ok(ToolResult { content: vec![Content::text("released")], details: None, terminate: false, ..Default::default() })
    }
}

struct BlockExt {
    tool: Arc<BlockTool>,
}
#[async_trait::async_trait]
impl NativeExtension for BlockExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("block-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(self.tool.clone());
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// gap #13: while the agent is streaming, `prompt_with(streaming_behavior)` EXPANDS the text and
/// queues it via steer/follow-up (instead of erroring / relegating to `send_user_message`).
#[tokio::test]
async fn prompt_with_streaming_behavior_expands_and_queues() {
    let fx = fixture();
    // A prompt template so we can prove the queued text was expanded.
    let prompts = fx.agent_dir.join("prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(prompts.join("greet.md"), "Hello from the template!").unwrap();

    let gate = Arc::new(Notify::new());
    let tool = Arc::new(BlockTool {
        gate: gate.clone(),
        params: serde_json::json!({"type": "object", "properties": {}}),
    });

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_tool_call("block", serde_json::json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("after tool")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("after steer")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(BlockExt { tool }))
        .build()
        .await
        .unwrap();

    // Start a run that calls the parking tool, then wait until the agent is actually streaming.
    let _stream = session.prompt("kick off").await.unwrap();
    let mut streaming = false;
    for _ in 0..400 {
        if session.is_streaming().await {
            streaming = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(streaming, "the parking tool must hold the agent in a streaming state");

    // Submit a `/greet` template while streaming with `steer`: it must be EXPANDED then queued.
    let accepted = session
        .prompt_with("/greet", PromptOptions { streaming_behavior: Some(StreamingBehavior::Steer) })
        .await
        .unwrap();
    assert_eq!(accepted, PromptAccepted::Queued(StreamingBehavior::Steer));
    let queued = session.steering_messages();
    assert_eq!(queued.len(), 1, "the steer queue mirrors the submission");
    assert!(
        queued[0].contains("Hello from the template!"),
        "the queued text was template-expanded before queueing, got: {:?}",
        queued[0]
    );

    // Without a behavior while streaming, the submission is rejected (Pi throws).
    let err = session.prompt_with("late", PromptOptions::default()).await.unwrap_err();
    assert!(matches!(err, cyrup_session_svc::SessionServiceError::StreamingNeedsBehavior));

    // Release the tool and let the run settle so the test does not leak a task.
    gate.notify_one();
    session.wait_for_idle().await;
}

// ==================================================================== #13 input extension event ====

/// A native extension that fully services every `input` event (Pi `action:"handled"`).
struct InputHandler;
#[async_trait::async_trait]
impl NativeExtension for InputHandler {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("input-handler")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { .. } => HookOutcome::Handled(HandledValue(serde_json::json!({
                "action": "handled"
            }))),
            _ => HookOutcome::Noop,
        }
    }
}

/// gap #13: an `input` handler that returns `handled` short-circuits the prompt — no run starts and
/// nothing is persisted (Pi agent-session.ts:1018-1028).
#[tokio::test]
async fn input_event_handled_short_circuits_prompt() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(InputHandler))
        .build()
        .await
        .unwrap();

    let accepted = session.prompt_with("anything", PromptOptions::default()).await.unwrap();
    assert_eq!(accepted, PromptAccepted::Handled, "the input handler serviced the submission");
    assert!(!session.is_streaming().await, "no run was started");
    assert!(session.messages().await.is_empty(), "nothing was persisted");
}

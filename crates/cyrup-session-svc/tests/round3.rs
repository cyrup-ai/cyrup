//! Round-3 facade parity tests (vs Pi `agent-session.ts`): the retry subsystem, auto-compaction
//! toggles, the immediate-bash seam, dynamic tools + custom tools, `setModel(Model)` + the typed
//! `cycleModel`/scoped models, the `prompt` ordering fix + skill/template expansion, `clone_at`, and
//! the runtime `modelFallbackMessage` getter.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{
    AssistantMessage, Content, StopReason, Tool, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, FauxConfig, FauxModelDefinition, FauxProvider,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    BashOptions, ScopedModel, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

fn two_model_provider() -> Arc<FauxProvider> {
    let mut reasoning = FauxModelDefinition::new("faux-2");
    reasoning.reasoning = true;
    let cfg = FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1"), reasoning],
        ..FauxConfig::default()
    };
    Arc::new(FauxProvider::with_config(cfg))
}

// A trivial custom tool (Pi `customTools`).
struct EchoTool {
    params: serde_json::Value,
}
impl EchoTool {
    fn new() -> Self {
        Self { params: serde_json::json!({"type": "object", "properties": {}}) }
    }
}
#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "Echo a message"
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Echo a message back")
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult { content: vec![Content::text("echo")], details: None, terminate: false })
    }
}

// ------------------------------------------------------------------------------ retry subsystem ----

#[tokio::test]
async fn retry_toggles_classification_and_backoff() {
    let fx = fixture();
    // Fast backoff so the success path completes quickly.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("retry", serde_json::json!({"enabled": true, "maxRetries": 2, "baseDelayMs": 3}))
        .unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).cli_settings(cli).build().await.unwrap();

    // Toggle mirrors the settings default, then the override.
    assert!(session.auto_retry_enabled(), "settings default retry.enabled = true");
    session.set_auto_retry_enabled(false);
    assert!(!session.auto_retry_enabled());
    session.set_auto_retry_enabled(true);

    // Classification: a transient error is retryable; a clean stop is not.
    let transient = AssistantMessage::errored(
        "faux".into(),
        "faux-1",
        None,
        StopReason::Error,
        "overloaded: please retry",
    );
    assert!(session.is_retryable_error(&transient), "overloaded is retryable");
    let clean = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    assert!(!session.is_retryable_error(&clean), "a clean stop is never retryable");

    // will_retry_after_agent_end scans the last assistant message.
    assert!(session
        .will_retry_after_agent_end(&[cyrup_agent::AgentMessage::Assistant(transient.clone())]));
    assert!(!session
        .will_retry_after_agent_end(&[cyrup_agent::AgentMessage::Assistant(clean.clone())]));

    // prepare_retry: first attempt waits the backoff and signals continue; the budget then exhausts.
    assert_eq!(session.retry_attempt(), 0);
    assert!(session.prepare_retry(&transient).await, "attempt 1 continues");
    assert_eq!(session.retry_attempt(), 1);
    assert!(session.prepare_retry(&transient).await, "attempt 2 continues");
    assert_eq!(session.retry_attempt(), 2);
    assert!(!session.prepare_retry(&transient).await, "budget exhausted at maxRetries");
    assert_eq!(session.retry_attempt(), 2, "attempt count is preserved on exhaustion");
    assert!(!session.is_retrying(), "no backoff is in flight after prepare returns");
}

// -------------------------------------------------------------------------- auto-compaction ----

#[tokio::test]
async fn auto_compaction_toggle_and_is_compacting() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    assert!(session.auto_compaction_enabled(), "settings default compaction.enabled = true");
    assert!(!session.is_compacting(), "nothing compacting at rest");
    session.set_auto_compaction_enabled(false);
    assert!(!session.auto_compaction_enabled());

    // With auto-compaction disabled, check_compaction is a no-op.
    let small = faux_assistant_message(vec![faux_text("hi")], StopReason::Stop);
    assert!(!session.check_compaction(&small, false).await.unwrap(), "disabled = never compacts");
    session.set_auto_compaction_enabled(true);
    // A tiny session is well under threshold → still no compaction.
    assert!(!session.check_compaction(&small, false).await.unwrap(), "small session under threshold");
}

// ------------------------------------------------------------------------------ bash seam ----

#[tokio::test]
async fn execute_bash_records_result_and_persists() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    assert!(!session.is_bash_running());
    assert!(!session.has_pending_bash_messages());
    let result = session
        .execute_bash("echo hello-bash", BashOptions::default(), None)
        .await
        .expect("a well-formed local echo command succeeds");
    assert_eq!(result.exit_code, Some(0), "echo exits 0");
    assert!(result.output.contains("hello-bash"), "captured stdout: {:?}", result.output);
    assert!(!result.cancelled);
    assert!(!session.is_bash_running(), "bash slot cleared after completion");

    // The bash result landed in the agent transcript (not streaming) as a bashExecution message.
    let msgs = session.agent_messages().await;
    assert!(
        msgs.iter().any(|m| matches!(m, cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "bashExecution")),
        "bash result recorded in transcript"
    );
    // abort_bash is idempotent when nothing runs.
    session.abort_bash();
}

// ---------------------------------------------------------------------------- dynamic tools ----

#[tokio::test]
async fn dynamic_tools_toggle_active_set_and_register_custom() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.custom_tools = vec![Arc::new(EchoTool::new())];
    let session = SessionBuilder::new(faux, cfg).build().await.unwrap();

    // The default active set is the built-in selection; the custom tool is enable-able but inactive.
    let active = session.active_tool_names();
    assert!(active.contains(&"read".to_string()), "read active by default: {active:?}");
    let all: Vec<String> = session.all_tools().into_iter().map(|t| t.name).collect();
    assert!(all.contains(&"echo".to_string()), "custom tool registered: {all:?}");
    assert!(session.tool_definition("echo").is_some());
    assert!(
        !session.active_tool_names().contains(&"echo".to_string()),
        "custom tool not auto-activated"
    );

    // Toggle the active set down to just read + echo; the agent's tool array follows.
    session.set_active_tools_by_name(&["read".to_string(), "echo".to_string()]).await;
    let active = session.active_tool_names();
    assert_eq!(active, vec!["read".to_string(), "echo".to_string()]);
    let snap = session.agent_messages().await; // force a snapshot to ensure no panic
    let _ = snap;
    // The agent's tool set now reflects the toggle.
    assert!(session.tool_definition("echo").unwrap().active, "echo is active after toggle");
    assert!(!session.tool_definition("write").map(|t| t.active).unwrap_or(false), "write toggled off");

    // Unknown names are ignored.
    session.set_active_tools_by_name(&["read".to_string(), "nope".to_string()]).await;
    assert_eq!(session.active_tool_names(), vec!["read".to_string()]);
}

// ------------------------------------------------------------------- model: set + cycle typed ----

#[tokio::test]
async fn set_model_resolved_auth_precheck_and_typed_cycle() {
    let fx = fixture();
    let faux = two_model_provider();
    let provider: Arc<dyn Provider> = faux.clone();
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    let session = SessionBuilder::new(provider.clone(), cfg).build().await.unwrap();

    // set_model_resolved on an in-catalog model succeeds.
    let faux2 = provider.models().iter().find(|m| m.id.as_str() == "faux-2").unwrap().clone();
    assert!(session.has_configured_auth(&faux2));
    session.set_model_resolved(faux2.clone()).await.unwrap();
    assert_eq!(session.model().model.as_str(), "faux-2");

    // A fabricated model not in the catalog fails the auth-proxy precheck.
    let mut bogus = faux2.clone();
    bogus.id = "ghost".into();
    assert!(!session.has_configured_auth(&bogus));
    assert!(session.set_model_resolved(bogus).await.is_err(), "out-of-catalog model rejected");

    // Typed cycle over the full catalog reports is_scoped = false.
    let r = session.cycle_model(true).await.unwrap().expect("two models cycle");
    assert!(!r.is_scoped, "no scoped set configured → available path");

    // Scoped set with a per-model thinking level reports is_scoped = true.
    session.set_scoped_models(vec![
        ScopedModel { model: provider.models()[0].clone(), thinking_level: None },
        ScopedModel { model: faux2.clone(), thinking_level: Some(cyrup_core::ModelThinkingLevel::High) },
    ]);
    let r = session.cycle_model(true).await.unwrap().expect("scoped cycle");
    assert!(r.is_scoped, "scoped set configured → scoped path");
}

// --------------------------------------------------------------- prompt ordering + expansion ----

#[tokio::test]
async fn prompt_injects_next_turn_after_user_and_expands_skill() {
    let fx = fixture();
    // A discoverable skill so `/skill:demo` expands.
    let dir = fx.agent_dir.join("skills").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo\ndescription: a demo skill\n---\n\nSKILL_BODY_MARKER\n",
    )
    .unwrap();

    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    // Stage a next-turn custom message; it must be injected AFTER the user message (Pi ordering).
    session
        .send_custom_message(
            "note",
            serde_json::json!({"text": "ctx"}),
            false,
            None,
            Some(cyrup_session_svc::DeliverAs::NextTurn),
        )
        .await
        .unwrap();

    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let _ = session.prompt("/skill:demo extra args").await.unwrap();
    session.wait_for_idle().await;

    let msgs = session.agent_messages().await;
    let user_idx = msgs.iter().position(|m| matches!(m, cyrup_agent::AgentMessage::User { .. }));
    let custom_idx = msgs
        .iter()
        .position(|m| matches!(m, cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "note"));
    let (u, c) = (user_idx.expect("user message present"), custom_idx.expect("next-turn custom present"));
    assert!(u < c, "user message must precede the injected next-turn message (Pi ordering): {u} < {c}");

    // The skill command expanded into the user message body.
    if let cyrup_agent::AgentMessage::User { content, .. } = &msgs[u] {
        let text: String = content
            .iter()
            .filter_map(|x| match x {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.contains("SKILL_BODY_MARKER"), "skill body expanded: {text}");
        assert!(text.contains("<skill name=\"demo\""), "skill block wrapper present");
        assert!(text.contains("extra args"), "trailing args preserved");
    } else {
        panic!("expected a user message at index {u}");
    }
}

// ------------------------------------------------------- clone_at + runtime fallback getter ----

#[tokio::test]
async fn clone_at_creates_new_file_and_runtime_surfaces_fallback() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider.clone(), base_config(&fx)).build().await.unwrap();

    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let _ = session.prompt("hi").await.unwrap();
    session.wait_for_idle().await;

    let original = session.session_id().clone();
    let cloned = session.clone_at(None).await.unwrap();
    assert_ne!(cloned, original, "clone_at branches into a distinct session id");

    // Runtime re-surfaces the (absent) model-fallback message of its active session.
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = cyrup_session_svc::AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap();
    assert!(runtime.model_fallback_message().await.is_none(), "clean model resolve = no fallback");
}

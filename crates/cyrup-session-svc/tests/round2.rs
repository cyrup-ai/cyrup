//! Round-2 facade parity tests (vs Pi `agent-session.ts` / `sdk.ts`): model+thinking
//! restore-from-session + seeding, thinking-level control, steering/follow-up mode control, the
//! `tools`/`excludeTools` selection, the `CompactionResult` flow, `import_from_jsonl`, and the
//! `deliverAs:nextTurn` custom-message staging.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_agent::QueueMode;
use cyrup_core::ModelThinkingLevel;
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, FauxConfig, FauxModelDefinition, FauxProvider,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSessionRuntime, DeliverAs, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
use cyrup_core::StopReason;
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

/// A provider with two models: `faux-1` (no reasoning) and `faux-2` (reasoning, full ladder).
fn two_model_provider() -> Arc<FauxProvider> {
    let mut reasoning = FauxModelDefinition::new("faux-2");
    reasoning.reasoning = true;
    let cfg = FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1"), reasoning],
        ..FauxConfig::default()
    };
    Arc::new(FauxProvider::with_config(cfg))
}

// ---------------------------------------------------------------- model + thinking restore ----

/// Pi sdk.ts:194-242,363-375: a NEW session seeds `model_change`/`thinking_level_change`; a resume
/// with no explicit `--model` restores the saved model (not the first-catalog default).
#[tokio::test]
async fn new_session_seeds_then_resume_restores_model() {
    let fx = fixture();
    let faux = two_model_provider();
    let provider: Arc<dyn Provider> = faux.clone();

    // New session pinned to faux-2 (NOT the first-catalog default).
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-2".to_string());
    let session = SessionBuilder::new(provider.clone(), cfg).build().await.unwrap();
    assert_eq!(session.model().model.as_str(), "faux-2");
    let file = session.session_file().await.expect("persisted session");
    // Drive a turn so the resumed session has messages (hasExistingSession) and the file flushes.
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let _ = session.prompt("hi").await.unwrap();
    session.wait_for_idle().await;
    // The seeded entries are on disk so a future resume can restore them.
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(on_disk.contains("\"type\":\"model_change\""), "model_change not seeded:\n{on_disk}");
    assert!(
        on_disk.contains("\"type\":\"thinking_level_change\""),
        "thinking_level_change not seeded"
    );
    drop(session);

    // Resume with NO model pattern: must restore faux-2, not fall back to first-catalog faux-1.
    let mut resume_cfg = base_config(&fx);
    resume_cfg.target = SessionTarget::Resume(file);
    let resumed = SessionBuilder::new(provider, resume_cfg).build().await.unwrap();
    assert_eq!(resumed.model().model.as_str(), "faux-2", "resume must restore the saved model");
    assert!(resumed.model_fallback_message().is_none(), "clean restore = no fallback message");
}

// ----------------------------------------------------------------------- thinking control ----

#[tokio::test]
async fn thinking_level_control_clamps_and_reports_support() {
    let fx = fixture();
    let faux = two_model_provider();
    let provider: Arc<dyn Provider> = faux.clone();

    // A reasoning model supports the ladder and persists changes.
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-2".to_string());
    let session = SessionBuilder::new(provider.clone(), cfg).build().await.unwrap();
    assert!(session.supports_thinking(), "faux-2 is a reasoning model");
    assert!(session.available_thinking_levels().len() > 1);
    let set = session.set_thinking_level(ModelThinkingLevel::High).await.unwrap();
    assert_eq!(set, ModelThinkingLevel::High);
    assert_eq!(session.thinking_level().await, ModelThinkingLevel::High);
    // Cycling advances to the next ladder level.
    let cycled = session.cycle_thinking_level().await.unwrap();
    assert!(cycled.is_some(), "reasoning model cycles");

    // A non-reasoning model only supports `off` and never cycles.
    let mut cfg1 = base_config(&fx);
    cfg1.model_pattern = Some("faux-1".to_string());
    let s1 = SessionBuilder::new(provider, cfg1).build().await.unwrap();
    assert!(!s1.supports_thinking());
    assert_eq!(s1.available_thinking_levels(), vec![ModelThinkingLevel::Off]);
    assert!(s1.cycle_thinking_level().await.unwrap().is_none(), "non-reasoning model never cycles");
}

// --------------------------------------------------------------- steering / follow-up mode ----

#[tokio::test]
async fn steering_and_follow_up_mode_control() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    // Default (from settings) is one-at-a-time.
    assert_eq!(session.steering_mode(), QueueMode::OneAtATime);
    assert_eq!(session.follow_up_mode(), QueueMode::OneAtATime);

    session.set_steering_mode(QueueMode::All);
    session.set_follow_up_mode(QueueMode::All);
    assert_eq!(session.steering_mode(), QueueMode::All);
    assert_eq!(session.follow_up_mode(), QueueMode::All);
}

// ----------------------------------------------------------------------- tool selection ----

#[tokio::test]
async fn tool_selection_allowlist_and_excludelist() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    // Allowlist: only `read` is active → the system prompt advertises read, not bash.
    let mut allow = base_config(&fx);
    allow.tools = Some(vec!["read".to_string()]);
    let s_allow = SessionBuilder::new(faux.clone(), allow).build().await.unwrap();
    let p = s_allow.system_prompt();
    assert!(p.contains("Read a file"), "read tool should be active:\n{p}");
    assert!(!p.contains("Run a shell command"), "bash should be excluded by the allowlist");

    // Denylist: exclude `bash` → its snippet disappears while others remain.
    let mut deny = base_config(&fx);
    deny.exclude_tools = vec!["bash".to_string()];
    let s_deny = SessionBuilder::new(faux, deny).build().await.unwrap();
    let pd = s_deny.system_prompt();
    assert!(pd.contains("Read a file"), "read should still be active");
    assert!(!pd.contains("Run a shell command"), "bash should be excluded by the denylist");
}

// ----------------------------------------------------------------------- compaction flow ----

#[tokio::test]
async fn compact_on_small_session_returns_none_and_emits_events() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();
    let mut sub = session.subscribe();

    // Nothing to compact on a fresh/tiny session: Ok(None), no panic, events still flow.
    let result = session.compact(None).await.expect("compact must not error");
    assert!(result.is_none(), "small session has nothing to compact");

    let mut saw_start = false;
    let mut saw_end = false;
    for _ in 0..6 {
        match tokio::time::timeout(std::time::Duration::from_millis(200), {
            use futures::StreamExt;
            sub.next()
        })
        .await
        {
            Ok(Some(ev)) => match ev.kind() {
                "compaction_start" => saw_start = true,
                "compaction_end" => saw_end = true,
                _ => {}
            },
            _ => break,
        }
    }
    assert!(saw_start && saw_end, "compaction_start + compaction_end must be emitted");
}

// ----------------------------------------------------------------------- import_from_jsonl ----

#[tokio::test]
async fn runtime_import_from_jsonl_switches_session() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("imported")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();

    // Build a source session with content and export it to a standalone JSONL file.
    let source = SessionBuilder::new(provider.clone(), base_config(&fx)).build().await.unwrap();
    let _ = source.prompt("seed message").await.unwrap();
    source.wait_for_idle().await;
    let export_path = fx.cwd.join("exported.jsonl");
    source.export_to_jsonl(Some(&export_path)).await.unwrap();
    assert!(export_path.exists());
    drop(source);

    // A fresh runtime imports the file and switches to it (not cancelled).
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();
    let result = runtime.import_from_jsonl(export_path, None).await.expect("import");
    assert!(!result.cancelled, "import must not be cancelled");
    let imported = runtime.session().await;
    let texts: Vec<String> = imported
        .messages()
        .await
        .iter()
        .filter_map(|m| match m {
            cyrup_core::Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "seed message"), "imported transcript missing: {texts:?}");

    // A missing source path is a typed error, not a panic.
    match runtime.import_from_jsonl(fx.cwd.join("nope.jsonl"), None).await {
        Err(cyrup_session_svc::SessionServiceError::ImportFileNotFound(_)) => {}
        other => panic!("expected ImportFileNotFound, got {other:?}"),
    }
}

// ------------------------------------------------------------- custom-message deliverAs ----

#[tokio::test]
async fn send_custom_message_next_turn_rides_the_next_prompt() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    // Stage a custom message to ride the NEXT turn.
    session
        .send_custom_message(
            "note",
            serde_json::json!({"hint": "remember"}),
            false,
            None,
            Some(DeliverAs::NextTurn),
        )
        .await
        .unwrap();

    // The next prompt carries the staged custom message into the run input.
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let agent_msgs = session.agent_messages().await;
    assert!(
        agent_msgs.iter().any(|m| matches!(m, cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "note")),
        "the next-turn custom message must ride the run input"
    );
}

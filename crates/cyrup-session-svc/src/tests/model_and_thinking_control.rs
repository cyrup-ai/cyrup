//! The session's TURN CONTROLS: the model + thinking-level pair, and the steering / follow-up
//! queue that decides what the next turn is made of.
//!
//! They are one file because they are the same seam seen from two sides — every dial here is a
//! setter/getter pair on the live `AgentSession` whose effect is only visible on the NEXT run, and
//! every one of them had the same failure mode: the value was accepted, stored, and then not read
//! by the run path.
//!
//! Model + thinking (Pi `sdk.ts:194-242,363-375`, `agent-session.ts` `setModel`/`cycleModel`): a
//! NEW session seeds `model_change`/`thinking_level_change`; a resume with no explicit `--model`
//! restores the SAVED model; the thinking ladder clamps to what the resolved model supports.
//!
//! The queue (Pi `agent-session.ts` steer/follow-up + `queue_update`): its modes, what may enter it
//! (`deliverAs:nextTurn` custom messages, in-prompt `streamingBehavior`, `/skill:` expansion), what
//! must be REFUSED (extension commands), and the `queue_update` a real drain emits.

use std::sync::Arc;
use std::time::Duration;

use cyrup_agent::QueueMode;
use cyrup_core::{
    Content, ExtensionId, ModelThinkingLevel, StopReason, Tool, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::{
    CommandDescriptor, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxConfig, FauxModelDefinition, FauxProvider,
};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::{
    AgentSessionEvent, DeliverAs, InputSource, PromptAccepted, PromptOptions, ScopedModel,
    SessionBuilder, SessionCommand, SessionCommandOutput, SessionServiceError, SessionTarget,
    StreamingBehavior, UserInput,
};
use futures::StreamExt;
use tokio::sync::Notify;

/// Stands in for the binary's `select_provider` seam: hands back an offline faux provider for any
/// id, so a cross-provider model change can actually install its owning provider.
struct AnyFauxResolver;

impl crate::ProviderResolver for AnyFauxResolver {
    fn resolve(&self, _provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        Ok(Arc::new(FauxProvider::new()))
    }
}

/// A provider with two models: `faux-1` (no reasoning) and `faux-2` (reasoning, full ladder).
/// A faux provider scripted with a single `ok` answer.
fn faux_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
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

// =========================================================== model + thinking level ====

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: model + thinking restore-from-session, and the seeding a NEW session does.
///
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
    assert_eq!(session.model().expect("session must have a resolved model").model.as_str(), "faux-2");
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
    assert_eq!(resumed.model().expect("session must have a resolved model").model.as_str(), "faux-2", "resume must restore the saved model");
    assert!(resumed.model_fallback_message().is_none(), "clean restore = no fallback message");
}

// ----------------------------------------------------------------------- thinking control ----

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: thinking-level control — the level clamps to what the resolved model actually
/// supports, and `supports_thinking`/`available_thinking_levels`/`cycle_thinking_level` report it.
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

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: steering / follow-up mode control (the `QueueMode` setters and their getters).
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

/// Facade parity vs Pi `agent-session.ts`: `setModel(Model)` + the typed `cycleModel`/scoped models — a resolved model is
/// auth-prechecked before it is installed, and the typed cycle walks the scoped model set.
#[tokio::test]
async fn set_model_resolved_auth_precheck_and_typed_cycle() {
    let fx = fixture();
    let faux = two_model_provider();
    let provider: Arc<dyn Provider> = faux.clone();
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    // The real host always carries a resolver (`main.rs` hands every factory a
    // `BuiltinProviderResolver`), and it is load-bearing for the cycle below: `cycle_model`'s
    // available arm walks `getAvailable()` across EVERY configured provider (Pi
    // `_modelRuntime.getAvailable()`, agent-session.ts:1644), and this fixture is not hermetic
    // against the ambient environment — a `TOGETHER_API_KEY` in the shell makes `together`
    // configured and puts its catalog in the cycle set, which then has to be installable.
    let session = SessionBuilder::new(provider.clone(), cfg)
        .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn crate::ProviderResolver>)
        .build()
        .await
        .unwrap();

    // set_model_resolved on an in-catalog model succeeds.
    let faux2 = provider.models().iter().find(|m| m.id.as_str() == "faux-2").unwrap().clone();
    assert!(session.has_configured_auth(&faux2));
    session.set_model_resolved(faux2.clone()).await.unwrap();
    assert_eq!(session.model().expect("session must have a resolved model").model.as_str(), "faux-2");

    // A fabricated model not in the catalog fails the auth-proxy precheck.
    let mut bogus = faux2.clone();
    bogus.id = "ghost".into();
    assert!(!session.has_configured_auth(&bogus));
    assert!(session.set_model_resolved(bogus).await.is_err(), "out-of-catalog model rejected");

    // Scoped set with a per-model thinking level reports is_scoped = true. Asserted BEFORE the
    // available arm because that arm now legitimately leaves the session on ANOTHER provider (it
    // walks every configured provider, Pi `_modelRuntime.getAvailable()`), which would put this
    // fixture's two scoped models out of the newly installed provider's catalog.
    session.set_scoped_models(vec![
        ScopedModel { model: provider.models()[0].clone(), thinking_level: None },
        ScopedModel { model: faux2.clone(), thinking_level: Some(cyrup_core::ModelThinkingLevel::High) },
    ]);
    let r = session.cycle_model(true).await.unwrap().expect("scoped cycle");
    assert!(r.is_scoped, "scoped set configured → scoped path");

    // Typed cycle over the available (auth-filtered) registry reports is_scoped = false.
    session.set_scoped_models(Vec::new());
    let r = session.cycle_model(true).await.unwrap().expect("two models cycle");
    assert!(!r.is_scoped, "no scoped set configured → available path");
}

// ================================================ what may ENTER the queue ====

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: the `deliverAs:nextTurn` custom-message staging — the staged message is held
/// until the next prompt and rides it, rather than being delivered on its own.
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

/// Facade parity vs Pi `agent-session.ts`: the `prompt` ordering fix + skill/template expansion — a staged next-turn
/// message is injected AFTER the user message (not before it), and a `/skill:` command in the
/// prompt is expanded before the run starts.
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
            Some(crate::DeliverAs::NextTurn),
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

/// A native extension registering a `/greet` command (so it is a known extension command).
struct GreetCommand;
#[async_trait::async_trait]
impl NativeExtension for GreetCommand {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("greet-command")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "greet",
            CommandDescriptor { description: "greet".into(), completions: vec![] },
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A.8, the reject half: `steer`/`follow_up` reject a queued EXTENSION command (Pi
/// `_throwIfExtensionCommand`, agent-session.ts:1242-1252/1262-1272), while a plain message queues
/// normally.
#[tokio::test]
async fn steer_and_follow_up_reject_extension_commands() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(GreetCommand))
        .build()
        .await
        .expect("build");

    let err = session.steer("/greet hi").await.expect_err("an extension command cannot be steered");
    assert!(
        matches!(&err, SessionServiceError::ExtensionCommandNotQueueable(n) if n == "greet"),
        "steer rejects the extension command by name, got: {err}"
    );
    let err = session
        .follow_up("/greet hi")
        .await
        .expect_err("an extension command cannot be followed-up");
    assert!(matches!(err, SessionServiceError::ExtensionCommandNotQueueable(_)));

    // A non-command message queues normally.
    session.steer("just a plain steer").await.expect("plain steer queues");
    assert_eq!(session.steering_messages(), vec!["just a plain steer".to_string()]);
}

/// A.8, the expand half: `steer` EXPANDS a `/skill:<name>` command before queueing (Pi
/// `_expandSkillCommand`, agent-session.ts:1249) — the mirror carries the expanded skill block plus
/// the trailing args.
#[tokio::test]
async fn steer_expands_skill_command_before_queueing() {
    let fx = fixture();
    // A user-tier skill (not trust-gated) discovered at $HOME/.agents/skills/foo.
    let home = fx._tmp.path().join("home");
    let skill_dir = home.join(".agents").join("skills").join("foo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: foo\ndescription: a skill\n---\n\nSKILL_BODY_MARKER\n",
    )
    .unwrap();

    let mut cfg = base_config(&fx);
    cfg.home = home;
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, cfg).build().await.expect("build");

    session.steer("/skill:foo trailing args").await.expect("steer queues");
    let queued = session.steering_messages();
    assert_eq!(queued.len(), 1, "the steer mirror has the one queued message");
    assert!(queued[0].contains("SKILL_BODY_MARKER"), "skill body expanded into the queued text: {:?}", queued[0]);
    assert!(queued[0].contains("trailing args"), "the trailing args ride along: {:?}", queued[0]);
    assert!(!queued[0].starts_with("/skill:"), "the raw command was replaced by its expansion");
}

// ==================================================== how the queue DRAINS ====

/// A native extension contributing a `block` tool whose execution parks on a gate, so the run can be
/// held in a streaming state while a steering message is queued.
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
        "parks until released"
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

/// A.4: the queue-drain → `queue_update` branch (Pi `_handleAgentEvent` head,
/// agent-session.ts:517-533) fires from a REAL run, emptying the mirror on delivery. A steered
/// message is queued mid-run (mirror = ["steered"]); when the agent delivers it as a new user turn
/// the subscriber drains the mirror and emits a `queue_update` with the message removed
/// (mirror = []).
#[tokio::test]
async fn queue_drain_emits_queue_update_from_a_real_run() {
    let fx = fixture();
    let gate = Arc::new(Notify::new());
    let tool = Arc::new(BlockTool {
        gate: gate.clone(),
        params: serde_json::json!({"type": "object", "properties": {}}),
    });
    let faux = Arc::new(FauxProvider::new());
    // Turn 1 parks on the block tool; after release the steered user message drives turn 2; turn 3 stops.
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
        .expect("build")
        .into_shared();

    let stream = session.prompt(UserInput::text("kick off", InputSource::Sdk)).await.expect("prompt");

    // Wait until the run is actually streaming (parked on the tool), then steer.
    let mut streaming = false;
    for _ in 0..400 {
        if session.is_streaming().await {
            streaming = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(streaming, "the block tool must hold the agent streaming");

    session.steer("steered").await.expect("steer accepted");
    assert_eq!(session.steering_messages(), vec!["steered".to_string()], "mirror holds the queued steer");

    // Release the tool so the steered message is delivered as a new user turn.
    gate.notify_one();
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    // The steered message was delivered as a user turn (draining the mirror), so the provider was hit
    // again beyond the initial tool-call turn.
    assert!(faux.call_count() >= 2, "the run continued past the parked tool turn");

    // A queue_update with the steer ENQUEUED (mirror=["steered"]) and a later one DRAINED (mirror=[]).
    let queue_updates: Vec<Vec<String>> = events
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::QueueUpdate { steering, .. } => Some(steering.clone()),
            _ => None,
        })
        .collect();
    assert!(
        queue_updates.iter().any(|s| s == &vec!["steered".to_string()]),
        "an enqueue queue_update carried the steered message: {queue_updates:?}"
    );
    assert!(
        queue_updates.iter().any(std::vec::Vec::is_empty),
        "a drain queue_update fired with the mirror emptied: {queue_updates:?}"
    );
    // The drain is observed AFTER the enqueue (the queue shrank).
    let enqueue_pos = queue_updates.iter().position(|s| s == &vec!["steered".to_string()]).unwrap();
    let drain_pos = queue_updates.iter().rposition(std::vec::Vec::is_empty).unwrap();
    assert!(drain_pos > enqueue_pos, "the drain follows the enqueue: {queue_updates:?}");
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
    assert!(matches!(err, crate::SessionServiceError::StreamingNeedsBehavior));

    // Release the tool and let the run settle so the test does not leak a task.
    gate.notify_one();
    session.wait_for_idle().await;
}

// ============================== how a model PATTERN resolves at build time ====

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

// ============================== the queue seen through `SessionCommand` ====

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

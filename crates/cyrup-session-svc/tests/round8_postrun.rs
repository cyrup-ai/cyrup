//! Round-8 ASSEMBLED-RUN proofs for the post-run execution loop (`_runAgentPrompt` /
//! `_handlePostAgentRun`, Pi agent-session.ts:973-1022). These do NOT hand-call `prepare_retry` /
//! `check_compaction`; they drive a REAL `AgentSession` turn to completion over the scripted
//! `FauxProvider` and assert that auto-retry, post-run auto-compaction, the `agent_end.willRetry`
//! payload, and `auto_retry_end{success}` ACTUALLY fire from the wired run path. The session is bound
//! via `into_shared()` exactly as the runtime / SDK / print-mode bind it in production.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{AssistantMessage, StopReason};
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, FauxMessageOptions, FauxProvider,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionEvent, InputSource, SessionBuilder, SessionConfig, UserInput};
use futures::StreamExt;
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

/// A near-instant retry backoff so the success path completes promptly.
fn fast_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("retry", serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 1}))
        .unwrap();
    cli
}

fn kinds(events: &[AgentSessionEvent]) -> Vec<&'static str> {
    events.iter().map(AgentSessionEvent::kind).collect()
}

// ============================================================================ A.1/A.2/A.3 retry ====

/// The CRITICAL proof: a retryable transient error from a COMPLETED turn is auto-retried by the
/// assembled run path (not a hand-called `prepare_retry`). The provider must be hit a SECOND time
/// (the continuation), `auto_retry_start` then `auto_retry_end{success:true}` must fire, and the
/// first `agent_end` must carry `willRetry:true`.
#[tokio::test]
async fn assembled_run_auto_retries_a_transient_error_then_recovers() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Turn 1 = retryable transient error; turn 2 (the continuation) = clean success.
    faux.set_responses(vec![
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions { error_message: Some("overloaded".into()), ..Default::default() },
        ),
        faux_assistant_message(vec![faux_text("recovered")], StopReason::Stop),
    ]);

    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(fast_retry_settings())
        .build()
        .await
        .expect("build")
        .into_shared(); // bind the self-handle: the post-run loop is now LIVE.

    let stream = session
        .prompt(UserInput::text("please do the thing", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The continuation actually happened in the assembled run: BOTH scripted responses consumed.
    assert_eq!(faux.call_count(), 2, "provider must be hit a second time (the auto-retry continuation)");

    // auto_retry_start fired from the completed turn (NOT a hand call).
    assert!(ks.contains(&"auto_retry_start"), "auto_retry_start must fire from the run: {ks:?}");

    // auto_retry_end{success:true} fired on the recovered message_end + the retry counter reset.
    let retry_end_success = events.iter().any(|e| {
        matches!(e, AgentSessionEvent::AutoRetryEnd { success: true, attempt: 1, .. })
    });
    assert!(retry_end_success, "auto_retry_end{{success:true, attempt:1}} must fire: {ks:?}");
    assert_eq!(session.retry_attempt(), 0, "retry counter resets on the successful continuation");

    // The FIRST agent_end carried willRetry:true; the LAST carried willRetry:false.
    let agent_ends: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::AgentEnd { will_retry, .. } => Some(*will_retry),
            _ => None,
        })
        .collect();
    assert_eq!(agent_ends.len(), 2, "two agent_end events (error turn + recovered turn): {ks:?}");
    assert!(agent_ends[0], "first agent_end.willRetry must be true (transient error pending retry)");
    assert!(!agent_ends[1], "final agent_end.willRetry must be false (clean success)");

    // The session settled on the recovered answer.
    assert_eq!(session.last_assistant_text().await.as_deref(), Some("recovered"));
}

// ============================================================================ A.1 post-run compact ====

/// A context-overflow error from a COMPLETED turn triggers post-run auto-compaction in the assembled
/// run path — `run_auto_compaction` (a previously-dead method) now fires `compaction_start{overflow}`
/// from a real turn, not a hand call.
#[tokio::test]
async fn assembled_run_triggers_post_run_overflow_compaction() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();

    // The overflow error must be attributed to the SAME model the session runs (Pi `_checkCompaction`
    // same-model guard), so build it from the live model address.
    let model = session.model().expect("session must have a resolved model");
    let overflow = AssistantMessage::errored(
        model.provider.clone(),
        model.model.as_str(),
        None,
        StopReason::Error,
        "context_length_exceeded",
    );
    faux.set_responses(vec![overflow]);

    assert!(session.auto_compaction_enabled(), "auto-compaction on by default");

    let stream = session
        .prompt(UserInput::text("overflow me", InputSource::Sdk))
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The post-run compaction PRODUCER fired from the assembled run, tagged `overflow`.
    let overflow_start = events.iter().any(|e| {
        serde_json::to_value(e)
            .ok()
            .and_then(|v| {
                Some(
                    v.get("type")?.as_str()? == "compaction_start"
                        && v.get("reason")?.as_str()? == "overflow",
                )
            })
            .unwrap_or(false)
    });
    assert!(overflow_start, "compaction_start{{reason:overflow}} must fire from the run: {ks:?}");
    // A retryable-error path was NOT taken (overflow is excluded from retry).
    assert!(!ks.contains(&"auto_retry_start"), "overflow must NOT be retried: {ks:?}");
}

// ============================================================================ unbound = legacy ====

/// An UNBOUND (plain by-value) session keeps the legacy single-turn behavior: the post-run loop does
/// not run, so a transient error is NOT auto-retried (the provider is hit exactly once) — this guards
/// the bound/unbound split that keeps existing by-value callers unchanged.
#[tokio::test]
async fn unbound_session_does_not_run_the_post_run_loop() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions { error_message: Some("overloaded".into()), ..Default::default() },
        ),
        faux_assistant_message(vec![faux_text("unreached")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(fast_retry_settings())
        .build()
        .await
        .expect("build"); // NOT bound — plain by-value session.

    let stream = session.prompt(UserInput::text("go", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    assert_eq!(faux.call_count(), 1, "unbound session runs a single turn (no post-run retry)");
    assert!(!kinds(&events).contains(&"auto_retry_start"), "no auto-retry on an unbound session");
}

// ============================================================================ R6 user_agents_dir ====

/// The session-svc builder plumbs `DiscoveryConfig.user_agents_dir = $HOME/.agents` (Pi
/// `userAgentsSkillsDir`, package-manager.ts:2286), so a skill placed at `$HOME/.agents/skills/<name>`
/// is discovered by the ASSEMBLED session as a user-tier source (and is not trust-gated).
#[tokio::test]
async fn builder_loads_user_tier_agents_skills() {
    let fx = fixture();
    // A distinct HOME with a user-tier `.agents/skills/userskill` skill.
    let home = fx._tmp.path().join("home");
    let skill_dir = home.join(".agents").join("skills").join("userskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: userskill\ndescription: a user-tier skill\n---\n\nUSER_SKILL_BODY\n",
    )
    .unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.home = home;
    cfg.trust_override = Some(false); // user-tier skills are NOT trust-gated.
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let catalog = session.slash_command_catalog();
    let has_user_skill = catalog.iter().any(|c| {
        c.get("name").and_then(serde_json::Value::as_str) == Some("skill:userskill")
    });
    assert!(has_user_skill, "user-tier ~/.agents/skills/userskill must be discovered: {catalog:?}");
}

// ============================================================================ A.6 session_info ====

/// `set_session_name` emits `session_info_changed { name }` to live subscribers (Pi
/// agent-session.ts:2714-2715) — previously it persisted the entry and emitted NOTHING.
#[tokio::test]
async fn set_session_name_emits_session_info_changed() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();

    let mut stream = session.subscribe();
    session.set_session_name("my session").await.expect("set name");

    let mut found: Option<Option<String>> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
        if let AgentSessionEvent::SessionInfoChanged { name } = &ev {
            found = Some(name.clone());
            break;
        }
    }
    assert_eq!(found, Some(Some("my session".to_string())), "session_info_changed{{name}} must fire");
    assert_eq!(session.session_name().await.as_deref(), Some("my session"));
}

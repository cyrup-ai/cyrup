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

use cyrup_core::{AssistantMessage, ExtensionId, StopReason};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, FauxMessageOptions, FauxProvider,
    FauxResponseStep,
};
use cyrup_provider::Provider;
use crate::{AgentSessionEvent, InputSource, SessionBuilder, SessionConfig, UserInput};
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

/// SEAM-112 — after a successful OVERFLOW compaction the interrupted turn must actually be RETRIED.
///
/// pi `agent-session.ts:2307-2317`, its own comment: *"The overflow response was persisted on
/// message_end before _checkCompaction() removed it from agent state. Rebuilding state from the new
/// compaction can restore that kept entry, leaving an assistant as the final message.
/// agent.continue() rejects that state, so remove the retriable error or truncated-length response
/// again before continuing the interrupted turn."*
///
/// cyrup's `run_auto_compaction` had no such re-drop, so the chain broke at its last link:
/// `check_compaction` dropped the trailing assistant, the compaction's re-seed pulled it back out
/// of the session file, `handle_post_agent_run` returned `true`, and `Agent::continue_run`
/// (`cyrup-agent/src/agent.rs:2004-2029`) saw `last_is_assistant` with both queues empty and
/// returned `ContinueFromAssistant` — which `drive_run` (`session.rs:797`) turns into a silent
/// `break`. Overflow recovery compacted and then simply stopped: the user's turn never ran.
///
/// **RED before the fix:** exactly ONE `agent_end` (the overflow turn), `call_count == 3`
/// (turn 1 + the overflow turn + the summarization) and no retried answer — the third scripted
/// response is never requested.
#[tokio::test]
async fn a_successful_overflow_compaction_retries_the_interrupted_turn() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    // keepRecentTokens/reserveTokens at 0 so the two-turn branch really has a preparation.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    // Turn 1: an ordinary answer, so the branch has something to compact.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("first answer worth some tokens")],
        StopReason::Stop,
    )]);
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;

    // Turn 2 overflows on the SAME model (pi's `_checkCompaction` same-model guard) with a
    // stop reason other than `Stop`, which is pi's `willRetry` predicate (`agent-session.ts:2032`):
    // `check_compaction` drops the trailing assistant and compacts with `willRetry: true`.
    // Response 2 is the summarization; response 3 is the answer the RETRY must fetch.
    let model = session.model().expect("session must have a resolved model");
    faux.set_responses(vec![
        AssistantMessage::errored(
            model.provider.clone(),
            model.model.as_str(),
            None,
            StopReason::Error,
            "context_length_exceeded",
        ),
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("RETRIED ANSWER")], StopReason::Stop),
    ]);

    let stream = session.prompt("tell me two").await.expect("prompt accepted");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    let ks = kinds(&events);

    // The compaction ran, succeeded, and carried pi's `willRetry` through to its end event.
    let end = events.iter().find_map(|e| match e {
        AgentSessionEvent::CompactionEnd { reason, result, will_retry, .. } => {
            Some((*reason, result.is_some(), *will_retry))
        }
        _ => None,
    });
    assert_eq!(
        end,
        Some((crate::CompactionReason::Overflow, true, true)),
        "the overflow compaction must SUCCEED and report willRetry:true: {ks:?}"
    );

    // …and the interrupted turn was then actually driven. Two `agent_end`s: the overflow turn and
    // the continuation.
    let agent_ends = ks.iter().filter(|k| **k == "agent_end").count();
    assert_eq!(
        agent_ends, 2,
        "overflow recovery must compact AND retry — one agent_end means `continue_run` refused the \
         restored trailing assistant (pi re-drops it, agent-session.ts:2312-2317): {ks:?}"
    );
    assert_eq!(
        faux.call_count(),
        4,
        "turn 1 + the overflow turn + the summarization + the RETRY; 3 means the retry never \
         reached the provider"
    );
    assert_eq!(
        session.last_assistant_text().await.as_deref(),
        Some("RETRIED ANSWER"),
        "the user's interrupted turn must end on the retried answer, not on the overflow error"
    );
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

/// A native extension that records every `session_info_changed` payload it is handed.
struct InfoChangedRecorder(Arc<std::sync::Mutex<Vec<Option<String>>>>);

#[async_trait::async_trait]
impl NativeExtension for InfoChangedRecorder {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("info-changed-recorder")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionInfoChanged]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::SessionInfoChanged { name } = ev {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push(name.clone());
        }
        HookOutcome::Noop
    }
}

/// EXT-011 — the rename is also an EXTENSION event: pi `SessionInfoChangedEvent`
/// (`extensions/types.ts:571-575` @v0.83.0), subscribed and dispatched like any other lifecycle
/// notify.
///
/// RED before this pass: `EventKind::SessionInfoChanged`, `HostEvent::SessionInfoChanged`, the WIT
/// export and the SDK's `on_session_info_changed` all existed, but NOTHING in the session emitted
/// it — `set_session_name` fanned the event out to `AgentSessionEvent` subscribers only. A guest
/// could subscribe and never be called, which is the worst failure shape: silent and untestable
/// from the guest side. This recorder would collect zero payloads.
#[tokio::test]
async fn set_session_name_also_dispatches_the_session_info_changed_extension_event() {
    let fx = fixture();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(InfoChangedRecorder(Arc::clone(&seen))))
        .build()
        .await
        .expect("build")
        .into_shared();

    session.set_session_name("my session").await.expect("set name");

    assert_eq!(
        seen.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec![Some("my session".to_string())],
        "the extension must receive `session_info_changed` with the resolved name"
    );

    // An empty/whitespace name resolves to `None` through `getSessionName()`, and the extension
    // sees the SAME `None` the `AgentSessionEvent` subscribers do.
    session.set_session_name("   ").await.expect("clear name");
    assert_eq!(
        seen.lock().unwrap_or_else(|e| e.into_inner()).last().cloned(),
        Some(None),
        "a blank rename dispatches `name: None`, not the previous name"
    );
}

/// CFG-006 / AGENT-031 — the `websocketConnectTimeoutMs` setting must reach the provider's
/// `StreamOptions`.
///
/// pi resolves it in the session `streamFn` as
/// `options?.websocketConnectTimeoutMs ?? settingsManager.getWebSocketConnectTimeoutMs()` and
/// spreads it onto every `streamSimple` call (`core/sdk.ts:310-311,314` @v0.83.0).
///
/// RED before this pass: BOTH halves existed and neither was connected —
/// `Settings::websocket_connect_timeout_ms` parsed and validated the key (`settings.rs:732`) and
/// `AgentBuilder::websocket_connect_timeout_ms` threaded it onto `StreamOptions`
/// (`cyrup-provider/src/stream.rs:201`), but nothing in `SessionBuilder` assigned it. A user who
/// set the key got no error and no effect, which is the AGENT-021 defect shape: a field documented
/// as live that silently sends nothing. The factory below would observe `None`.
#[tokio::test]
async fn websocket_connect_timeout_setting_reaches_the_providers_stream_options() {
    let fx = fixture();
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("websocketConnectTimeoutMs", serde_json::json!(7_500)).unwrap();

    let seen: Arc<std::sync::Mutex<Vec<Option<u64>>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::factory({
        let seen = Arc::clone(&seen);
        move |_ctx, options, _state, _model| {
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(options.websocket_connect_timeout_ms);
            faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
        }
    })]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    let _ = session.prompt(UserInput::text("go", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;

    assert_eq!(
        seen.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec![Some(7_500)],
        "the resolved `websocketConnectTimeoutMs` must arrive on `StreamOptions` for every request"
    );
}

/// CFG-035 — `.cyrup/SYSTEM.md` / `.cyrup/APPEND_SYSTEM.md` must actually be READ.
///
/// pi `discoverSystemPromptFile` / `discoverAppendSystemPromptFile`
/// (`core/resource-loader.ts:1022-1032`, `:1034-1044` @v0.83.0), consumed at `:525` and `:531-535`:
/// the project file `<cwd>/.cyrup/<name>` wins when the project is TRUSTED, else the global
/// `<agent_dir>/<name>`, else nothing — and exactly ONE path is returned per leg.
///
/// RED before this pass: both filenames existed in cyrup only as TRUST-GATE MARKERS
/// (`cyrup-config/src/trust.rs:208-209`). `SessionBuilder` set `custom_prompt` /
/// `append_system_prompt` from the CLI fields and nothing else, so cyrup asked the user to trust a
/// project *because of* a file it never opened: the assembled prompt would carry neither string.
#[tokio::test]
async fn system_md_and_append_system_md_are_discovered_and_read() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".cyrup")).unwrap();
    std::fs::write(fx.cwd.join(".cyrup/SYSTEM.md"), "PROJECT-SYSTEM-BODY").unwrap();
    std::fs::write(fx.cwd.join(".cyrup/APPEND_SYSTEM.md"), "PROJECT-APPEND-BODY").unwrap();
    // A global pair too, to pin the PRECEDENCE: the trusted project file must win.
    std::fs::write(fx.agent_dir.join("SYSTEM.md"), "GLOBAL-SYSTEM-BODY").unwrap();

    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(
        prompt.contains("PROJECT-SYSTEM-BODY"),
        "the trusted project `.cyrup/SYSTEM.md` must REPLACE the default body: {prompt}"
    );
    assert!(
        !prompt.contains("GLOBAL-SYSTEM-BODY"),
        "the project file wins outright; upstream returns on the first hit and does not stack"
    );
    assert!(
        prompt.contains("PROJECT-APPEND-BODY"),
        "`.cyrup/APPEND_SYSTEM.md` must be appended: {prompt}"
    );
}

/// CFG-035, the trust gate: the gate applies to the PROJECT rung ONLY, so an untrusted project
/// falls THROUGH to the global `<agent_dir>/SYSTEM.md` rather than yielding nothing
/// (`resource-loader.ts:1023-1030` — the `existsSync(globalPath)` rung is outside the
/// `isProjectTrusted()` guard).
#[tokio::test]
async fn an_untrusted_project_falls_through_to_the_global_system_md() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".cyrup")).unwrap();
    std::fs::write(fx.cwd.join(".cyrup/SYSTEM.md"), "PROJECT-SYSTEM-BODY").unwrap();
    std::fs::write(fx.agent_dir.join("SYSTEM.md"), "GLOBAL-SYSTEM-BODY").unwrap();

    let mut cfg = base_config(&fx);
    cfg.trust_override = Some(false);
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(
        prompt.contains("GLOBAL-SYSTEM-BODY"),
        "an untrusted project must still get the GLOBAL file: {prompt}"
    );
    assert!(
        !prompt.contains("PROJECT-SYSTEM-BODY"),
        "the untrusted project file must not be read"
    );
}

/// CFG-035 — an explicit CLI `--system-prompt` / `--append-system-prompt` SUPPRESSES discovery
/// (`this.systemPromptSource ?? this.discoverSystemPromptFile()`, `:525`; and `if (!appendSources)`
/// at `:531`). pi does not stack the CLI value on top of the discovered file.
#[tokio::test]
async fn a_cli_prompt_suppresses_discovery_rather_than_stacking() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".cyrup")).unwrap();
    std::fs::write(fx.cwd.join(".cyrup/SYSTEM.md"), "PROJECT-SYSTEM-BODY").unwrap();
    std::fs::write(fx.cwd.join(".cyrup/APPEND_SYSTEM.md"), "PROJECT-APPEND-BODY").unwrap();

    let mut cfg = base_config(&fx);
    cfg.system_prompt = Some("CLI-SYSTEM-BODY".to_string());
    cfg.append_system_prompt = Some("CLI-APPEND-BODY".to_string());
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, cfg).build().await.expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(prompt.contains("CLI-SYSTEM-BODY"), "{prompt}");
    assert!(prompt.contains("CLI-APPEND-BODY"), "{prompt}");
    assert!(!prompt.contains("PROJECT-SYSTEM-BODY"), "discovery must be suppressed: {prompt}");
    assert!(!prompt.contains("PROJECT-APPEND-BODY"), "discovery must be suppressed: {prompt}");
}

/// EXT-038 / TOOL-021 — an extension-contributed tool's `promptSnippet` and `promptGuidelines`
/// must reach the SYSTEM PROMPT.
///
/// pi builds `_toolPromptSnippets` / `_toolPromptGuidelines` from `definitionRegistry`, which is
/// the base definitions with `allCustomTools` merged over them by name
/// (`core/agent-session.ts:2471-2504` @v0.83.0) — so an extension tool contributes its own text,
/// and an extension OVERRIDE of a built-in contributes the override's text instead of the
/// built-in's.
///
/// RED before this pass, for two independent reasons:
/// 1. `SessionBuilder` derived `selected_tools` + `tool_contributions` from `base_tools` alone and
///    only called `ext_host.active_tools(&base_tools)` AFTER the prompt had been built, so a guest
///    could register a fully-described tool and the model was never told it existed;
/// 2. `Tool::prompt_guidelines` returned `&[&str]`, which no tool owning `Vec<String>` can
///    implement — so even with the ordering fixed the guidelines leg had no reader (TOOL-021).
#[tokio::test]
async fn an_extension_tools_snippet_and_guidelines_reach_the_system_prompt() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(DescribedToolExtension))
        .build()
        .await
        .expect("build");

    let prompt = session.system_prompt().to_string();
    assert!(
        prompt.contains("Deploys the thing to the place"),
        "the extension tool's `promptSnippet` must reach the Available tools section: {prompt}"
    );
    assert!(
        prompt.contains("Always dry-run deploy before deploying for real"),
        "the extension tool's `promptGuidelines` must reach the Guidelines section: {prompt}"
    );
}

/// A native extension contributing one tool with a snippet AND owned (`String`) guidelines — the
/// same ownership shape a WASM guest's `ToolDescriptor` has.
struct DescribedToolExtension;

#[async_trait::async_trait]
impl NativeExtension for DescribedToolExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("described-tool-extension")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(DeployTool {
            params: serde_json::json!({"type": "object", "properties": {}}),
            guidelines: vec!["Always dry-run deploy before deploying for real".to_string()],
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

struct DeployTool {
    params: serde_json::Value,
    guidelines: Vec<String>,
}

#[async_trait::async_trait]
impl cyrup_core::Tool for DeployTool {
    fn name(&self) -> &str {
        "deploy"
    }
    fn description(&self) -> &str {
        "deploy things"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Deploys the thing to the place")
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.guidelines.iter().map(String::as_str).collect()
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _params: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult::default())
    }
}

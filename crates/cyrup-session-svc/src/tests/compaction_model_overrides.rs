//! SESS-055 / CFG-082 — per-model compaction budgets (`compaction.modelOverrides`, pi v0.86.0).
//!
//! pi resolves `reserveTokens`/`keepRecentTokens` through the exact `"provider/modelId"` entry of
//! `compaction.modelOverrides`, then the ordinary setting, then the default, and THROWS on an
//! invalid value — `getCompactionTokenSetting` (`settings-manager.ts:859-885` @v0.87.1), read via
//! `getCompactionSettings(model)` at every compaction call site in `agent-session.ts` (`:589`,
//! `:2419`, `:2604`, `:2749`). These tests drive those call sites through the session facade.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig};
use cyrup_agent::AgentMessage;
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

fn compaction_settings(compaction: serde_json::Value) -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("compaction", compaction).unwrap();
    cli
}

/// Two answered prompts, then a manual `/compact`; returns what `compact` returned.
async fn two_turns_then_compact(compaction: serde_json::Value) -> Result<String, String> {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .cli_settings(compaction_settings(compaction))
        .build()
        .await
        .expect("build");
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    session
        .compact(None)
        .await
        .map(|r| r.summary)
        .map_err(|e| e.to_string())
}

/// The override for the ACTIVE model (`faux/faux-1`) governs the manual compaction's
/// `keepRecentTokens`; an override keyed to any other model is ignored and the global budget —
/// which keeps the whole two-turn session — applies.
#[tokio::test]
async fn the_active_models_override_budget_governs_manual_compaction() {
    let with_override = |key: &str| {
        serde_json::json!({
            "enabled": true,
            "reserveTokens": 0,
            "keepRecentTokens": 1_000_000,
            "modelOverrides": { key: { "keepRecentTokens": 0 } },
        })
    };
    let summary = two_turns_then_compact(with_override("faux/faux-1"))
        .await
        .expect("the faux/faux-1 override keeps nothing, so there is history to summarize");
    assert!(summary.contains("CONTEXT SUMMARY"), "{summary}");

    let err = two_turns_then_compact(with_override("faux/another-model"))
        .await
        .expect_err("another model's override must not apply");
    assert_eq!(err, "Nothing to compact (session too small)");
}

/// An invalid override for the active model is pi's thrown `Error`, caught by `compact()`'s own
/// handler: the call fails with pi's message and `compaction_end` carries
/// `"Compaction failed: …"` (`agent-session.ts:2419` inside the `try`).
#[tokio::test]
async fn an_invalid_override_fails_manual_compaction_with_pis_message() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("answer")],
        StopReason::Stop,
    )]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .cli_settings(compaction_settings(serde_json::json!({
            "enabled": false,
            "modelOverrides": { "faux/faux-1": { "reserveTokens": -5 } },
        })))
        .build()
        .await
        .expect("build")
        .into_shared();
    let expected = "Invalid compaction.modelOverrides[\"faux/faux-1\"].reserveTokens setting: -5. \
                    Expected a non-negative safe integer.";

    let mut stream = session.subscribe();
    let err = session
        .compact(None)
        .await
        .expect_err("pi throws from getCompactionSettings");
    assert_eq!(err.to_string(), expected);
    let mut end = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
        if let AgentSessionEvent::CompactionEnd { error_message, .. } = ev {
            end = error_message;
            break;
        }
    }
    assert_eq!(
        end.as_deref(),
        Some(format!("Compaction failed: {expected}").as_str())
    );
}

/// pi's pre-prompt `_checkCompaction(lastAssistant, false)` reads the settings BEFORE its enable
/// check (`agent-session.ts:1697`, `:2604`), so an invalid ordinary budget refuses the next prompt
/// even with auto-compaction switched off.
#[tokio::test]
async fn an_invalid_ordinary_budget_refuses_the_next_prompt_even_when_disabled() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .cli_settings(compaction_settings(
            serde_json::json!({ "enabled": false, "keepRecentTokens": 2.5 }),
        ))
        .build()
        .await
        .expect("build");
    // No assistant turn yet: pi skips the pre-prompt check, so the first prompt runs.
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;

    let Err(err) = session.prompt("tell me two").await else {
        panic!("the pre-prompt compaction check throws");
    };
    assert_eq!(
        err.to_string(),
        "Invalid compaction.keepRecentTokens setting: 2.5. Expected a non-negative safe integer."
    );
}

/// A tool whose result gives the context some size.
struct Bulky(serde_json::Value);
#[async_trait::async_trait]
impl cyrup_core::Tool for Bulky {
    fn name(&self) -> &str {
        "bulky"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.0
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _params: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult {
            content: vec![cyrup_core::Content::text("payload ".repeat(400))],
            ..Default::default()
        })
    }
}

/// Run one `go` prompt whose first turn calls `bulky` (so the loop reaches a turn boundary) and
/// return every event up to `agent_settled`.
async fn run_bulky_loop(compaction: serde_json::Value) -> Vec<AgentSessionEvent> {
    use cyrup_provider::faux::faux_tool_call;
    let step = |call: bool| {
        FauxResponseStep::factory(move |_ctx, _opts, _state, _model| {
            if call {
                faux_assistant_message(
                    vec![faux_tool_call("bulky".to_string(), serde_json::json!({}))],
                    StopReason::ToolUse,
                )
            } else {
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            }
        })
    };
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![step(true), step(false), step(false), step(false)]);

    let fx = fixture();
    let mut cfg = config(&fx);
    cfg.custom_tools = vec![Arc::new(Bulky(
        serde_json::json!({ "type": "object", "properties": {} }),
    )) as Arc<dyn cyrup_core::Tool>];
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .cli_settings(compaction_settings(compaction))
        .build()
        .await
        .unwrap()
        .into_shared();
    session
        .set_active_tools_by_name(&["bulky".to_string()])
        .await;

    let mut stream = session.subscribe();
    let running = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            let _ = session.prompt("go").await;
            session.wait_for_idle().await;
        }
    });
    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        let settled = matches!(ev, AgentSessionEvent::AgentSettled);
        events.push(ev);
        if settled {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(20), running).await;
    events
}

/// The turn-boundary threshold (`_compactBeforeNextAssistantResponse`, `agent-session.ts:588-605`)
/// uses the active model's `reserveTokens` override: a global reserve of 0 never crosses the
/// threshold, the `faux/faux-1` override of 127 999 does, and the compaction runs INSIDE the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_turn_boundary_threshold_reads_the_active_models_reserve_override() {
    let events = run_bulky_loop(serde_json::json!({
        "enabled": true,
        "keepRecentTokens": 0,
        "reserveTokens": 0,
        "modelOverrides": { "faux/faux-1": { "reserveTokens": 127_999 } },
    }))
    .await;
    let kinds: Vec<&str> = events.iter().map(AgentSessionEvent::kind).collect();
    let agent_end_at = kinds
        .iter()
        .position(|k| *k == "agent_end")
        .unwrap_or(kinds.len());
    assert!(
        kinds[..agent_end_at].contains(&"compaction_start"),
        "the override's reserve must trigger the turn-boundary compaction: {kinds:?}"
    );
}

/// An invalid budget at the turn boundary is a throw from pi's `prepareNextTurnWithContext`,
/// which the agent turns into a failed run (`handleRunFailure`, `agent.ts:528-543`): an assistant
/// `message_end` with `stopReason: "error"` and pi's message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_budget_at_the_turn_boundary_fails_the_run_with_pis_message() {
    let events = run_bulky_loop(serde_json::json!({
        "enabled": true,
        "modelOverrides": { "faux/faux-1": "small" },
    }))
    .await;
    let failed = events.iter().find_map(|ev| match ev {
        AgentSessionEvent::MessageEnd {
            message: AgentMessage::Assistant(a),
        } if a.stop_reason == StopReason::Error => a.error_message.clone(),
        _ => None,
    });
    assert_eq!(
        failed.as_deref(),
        Some(
            "Invalid compaction.modelOverrides[\"faux/faux-1\"] setting: small. Expected an object."
        ),
        "{:?}",
        events
            .iter()
            .map(AgentSessionEvent::kind)
            .collect::<Vec<_>>()
    );
}

/// Pi's `_runAutoCompaction` never throws a failed summary: its `catch` reports it on
/// `compaction_end.errorMessage` and `return false` (`agent-session.ts:2873-2896` @v0.87.1), so
/// the settings read is the only error a compaction check can raise. A threshold compaction whose
/// summary fails at the pre-prompt check therefore does not refuse the prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_auto_compaction_summary_does_not_refuse_the_next_prompt() {
    use cyrup_provider::faux::{FauxMessageOptions, faux_assistant_message_with};
    let refused = || {
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions {
                error_message: Some("summary refused".into()),
                ..Default::default()
            },
        )
    };
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        // The post-run threshold compaction after the first answer.
        refused(),
        // The pre-prompt threshold compaction ahead of the second prompt.
        refused(),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        refused(),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .cli_settings(compaction_settings(serde_json::json!({
            "enabled": true,
            "keepRecentTokens": 0,
            "reserveTokens": 127_999,
        })))
        .build()
        .await
        .expect("build")
        .into_shared();
    let mut stream = session.subscribe();
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;

    let _ = session
        .prompt("tell me two")
        .await
        .expect("a failed auto-compaction summary must not refuse the prompt");
    session.wait_for_idle().await;

    let mut failures = Vec::new();
    let mut second_answer = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
        match ev {
            AgentSessionEvent::CompactionEnd {
                error_message: Some(e),
                ..
            } => failures.push(e),
            AgentSessionEvent::MessageEnd {
                message: AgentMessage::Assistant(a),
            } if format!("{:?}", a.content).contains("second answer") => second_answer = true,
            _ => {}
        }
    }
    assert!(second_answer, "the second prompt must reach the model");
    assert!(
        failures.len() >= 2
            && failures
                .iter()
                .all(|e| e.starts_with("Auto-compaction failed: ")),
        "{failures:?}"
    );
}

/// A native built-in that queues one follow-up from its first `agent_end` handler — the
/// "messages queued by agent_end handlers" pi's post-run loop continues for
/// (`agent-session.ts:1525-1527` @v0.87.1).
struct QueueOnAgentEnd {
    session: Arc<std::sync::OnceLock<std::sync::Weak<crate::AgentSession>>>,
    queued: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for QueueOnAgentEnd {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("queue-on-agent-end")
    }

    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::AgentEnd]);
        Ok(())
    }

    async fn on_event(
        &self,
        ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        if matches!(ev, cyrup_ext::HostEvent::AgentEnd { .. })
            && !self.queued.swap(true, std::sync::atomic::Ordering::SeqCst)
            && let Some(session) = self.session.get().and_then(std::sync::Weak::upgrade)
        {
            session
                .follow_up("queued by agent_end")
                .await
                .expect("follow_up");
        }
        cyrup_ext::HookOutcome::Noop
    }
}

/// The post-run `_checkCompaction` throw (`agent-session.ts:2604`) leaves `_runAgentPrompt`'s
/// loop, so a follow-up an `agent_end` handler queued is NOT continued for: the run settles after
/// one model call and the follow-up stays queued. cyrup's post-run driver used to read the error as
/// "no compaction" and fall through to the queued-message continuation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_budget_after_the_run_stops_the_loop_before_a_queued_follow_up() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("continued answer")], StopReason::Stop),
    ]);
    let slot = Arc::new(std::sync::OnceLock::new());
    let ext = Arc::new(QueueOnAgentEnd {
        session: Arc::clone(&slot),
        queued: std::sync::atomic::AtomicBool::new(false),
    });
    let session = SessionBuilder::new(Arc::clone(&faux) as Arc<dyn Provider>, config(&fx))
        .cli_settings(compaction_settings(
            serde_json::json!({ "enabled": true, "reserveTokens": -1 }),
        ))
        .with_native_extension(ext as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    slot.set(Arc::downgrade(&session)).unwrap();

    // No assistant turn yet, so there is no pre-prompt check to throw; the post-run one does.
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;

    assert_eq!(faux.call_count(), 1, "no continuation after the throw");
    assert_eq!(
        session.follow_up_messages(),
        vec!["queued by agent_end".to_string()]
    );
}

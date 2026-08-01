//! Round-9 L5-residual proofs for cyrup-session-svc (vs Pi `agent-session.ts` + `compaction.ts`).
//!
//! Each test drives a REAL `AgentSession` (over the scripted `FauxProvider`) — no hand-called private
//! methods for the dispatch-producer proof — and asserts the closed residuals:
//!   * A.7  `compaction_end` carries the full Pi payload `{reason,result,aborted,willRetry,errorMessage?}`.
//!   * A.8  `steer`/`follow_up` expand skill/template AND reject extension commands (`_throwIfExtensionCommand`).
//!   * A.4  the queue-drain → `queue_update` branch fires from a real run (mirror emptied on delivery).
//!   * check_compaction Case-2 (threshold, direct-usage) fires from a real run.
//!   * B/user_bash the `user_bash` ext event fires with the LIVE `{command, excludeFromContext, cwd}`
//!     from `execute_bash_interactive` only — never from the RPC-reachable bare `execute_bash`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{
    Content, ExtensionId, StopReason, Tool, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{
    CommandDescriptor, EventKind, EventPatch, ExtError, HostCtx, HostEvent, HookOutcome, InitApi,
    NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSessionEvent, BashOptions, InputSource, SessionBuilder, SessionConfig, SessionServiceError,
    UserInput,
};
use futures::StreamExt;
use tempfile::TempDir;
use tokio::sync::Notify;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    home: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir, home }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

fn faux_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// Compaction settings that force even a small session to compact (keep nothing, reserve nothing).
fn aggressive_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

// ============================================================ A.7 compaction_end payload shape ====

/// A.7: a real (driven) MANUAL compaction emits `compaction_end` carrying the FULL Pi payload — the
/// `result` object (summary/firstKeptEntryId/tokensBefore/estimatedTokensAfter), `aborted:false`,
/// `willRetry:false`, and NO `errorMessage` key (Pi agent-session.ts:142-148 / 2062-2069).
#[tokio::test]
async fn compaction_end_carries_full_pi_payload() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Two real turns to build a transcript, then the compaction summary completion.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        // The compaction may issue a split-turn (history + turn-prefix) pair of summaries; supply
        // ample summary completions so summarization never starves.
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let mut sub = session.subscribe();
    let _result = session
        .compact(None)
        .await
        .expect("an aggressive-keep compaction over two turns produces a result");

    // Find the compaction_end on the live stream and assert its serialized shape.
    let mut end: Option<serde_json::Value> = None;
    for _ in 0..12 {
        match tokio::time::timeout(Duration::from_millis(300), sub.next()).await {
            Ok(Some(ev)) => {
                if ev.kind() == "compaction_end" {
                    end = Some(serde_json::to_value(&ev).unwrap());
                    break;
                }
            }
            _ => break,
        }
    }
    let v = end.expect("compaction_end must be emitted");
    assert_eq!(v["type"], "compaction_end");
    assert_eq!(v["reason"], "manual");
    assert_eq!(v["aborted"], serde_json::json!(false));
    assert_eq!(v["willRetry"], serde_json::json!(false), "manual compaction never retries");
    assert!(v.get("errorMessage").is_none(), "no errorMessage on a clean compaction: {v}");
    let r = v.get("result").expect("result present on a successful compaction");
    assert!(r.get("summary").and_then(|s| s.as_str()).is_some(), "result.summary present: {r}");
    assert!(r.get("firstKeptEntryId").is_some(), "result.firstKeptEntryId present: {r}");
    assert!(r.get("tokensBefore").is_some(), "result.tokensBefore present: {r}");
    assert!(r.get("estimatedTokensAfter").is_some(), "result.estimatedTokensAfter present: {r}");
}

/// check_compaction Case-2 (threshold, direct-usage) + A.7: a real BOUND run whose assistant usage
/// exceeds `window − reserve` triggers post-run auto-compaction tagged `threshold`, and its
/// `compaction_end` carries `willRetry:false` (Pi agent-session.ts:1900-1927 / 2069).
#[tokio::test]
async fn real_run_threshold_compaction_emits_threshold_end() {
    let fx = fixture();
    // reserveTokens just below the window so any real usage trips the threshold.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 127999}),
    )
    .unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a real answer worth some tokens")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("THRESHOLD SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    let stream = session.prompt(UserInput::text("go", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    let starts: Vec<String> = events
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .filter(|v| v["type"] == "compaction_start")
        .filter_map(|v| v["reason"].as_str().map(str::to_string))
        .collect();
    assert_eq!(starts, vec!["threshold".to_string()], "exactly one threshold compaction from the run");

    let end = events
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .find(|v| v["type"] == "compaction_end")
        .expect("compaction_end must fire from the real run");
    assert_eq!(end["reason"], "threshold");
    assert_eq!(end["willRetry"], serde_json::json!(false), "a threshold compaction does not retry");
}

// ================================================================= A.8 steer expansion + guard ====

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

/// A.8: `steer`/`follow_up` reject a queued EXTENSION command (Pi `_throwIfExtensionCommand`,
/// agent-session.ts:1242-1252/1262-1272), while a plain message queues normally.
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

/// A.8: `steer` EXPANDS a `/skill:<name>` command before queueing (Pi `_expandSkillCommand`,
/// agent-session.ts:1249) — the mirror carries the expanded skill block + the trailing args.
#[tokio::test]
async fn steer_expands_skill_command_before_queueing() {
    let fx = fixture();
    // A user-tier skill (not trust-gated) discovered at $HOME/.agents/skills/foo.
    let skill_dir = fx.home.join(".agents").join("skills").join("foo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: foo\ndescription: a skill\n---\n\nSKILL_BODY_MARKER\n",
    )
    .unwrap();

    let mut cfg = base_config(&fx);
    cfg.home = fx.home.clone();
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, cfg).build().await.expect("build");

    session.steer("/skill:foo trailing args").await.expect("steer queues");
    let queued = session.steering_messages();
    assert_eq!(queued.len(), 1, "the steer mirror has the one queued message");
    assert!(queued[0].contains("SKILL_BODY_MARKER"), "skill body expanded into the queued text: {:?}", queued[0]);
    assert!(queued[0].contains("trailing args"), "the trailing args ride along: {:?}", queued[0]);
    assert!(!queued[0].starts_with("/skill:"), "the raw command was replaced by its expansion");
}

// ================================================================= A.4 queue-drain → queue_update ==

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
        Ok(ToolResult { content: vec![Content::text("released")], details: None, terminate: false })
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

/// A.4: the queue-drain → `queue_update` branch (Pi `_handleAgentEvent` head, agent-session.ts:517-
/// 533) fires from a REAL run. A steered message is queued mid-run (mirror = ["steered"]); when the
/// agent delivers it as a new user turn the subscriber drains the mirror and emits a `queue_update`
/// with the message removed (mirror = []).
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

// ===================================================================== B/user_bash live values ====

type BashProbe = Arc<Mutex<Vec<(String, bool, String)>>>;

/// A native extension that records every `user_bash` event payload it is delivered.
struct UserBashProbe(BashProbe);
#[async_trait::async_trait]
impl NativeExtension for UserBashProbe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("user-bash-probe")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::UserBash]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::UserBash { command, exclude_from_context, cwd } = ev {
            self.0.lock().unwrap().push((command.clone(), *exclude_from_context, cwd.clone()));
        }
        HookOutcome::Noop
    }
}

/// B/user_bash: `execute_bash_interactive` (the interactive `!`/`!!`-prefix entry point) fires the
/// `user_bash` extension event from the submission pipeline with the LIVE `{command,
/// excludeFromContext (the !!-prefix flag), cwd (agent cwd)}` (Pi `extensionRunner.emitUserBash`,
/// `interactive-mode.ts:5663-5669`'s `handleBashCommand` / `types.ts:782-790`). The bare
/// `execute_bash` (which the JSON-RPC `bash` command calls directly, `rpc-mode.ts:550-554`) fires no
/// such event — see `execute_bash_never_emits_user_bash` below.
#[tokio::test]
async fn execute_bash_interactive_emits_user_bash_with_live_values() {
    let fx = fixture();
    let probe: BashProbe = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(UserBashProbe(probe.clone())))
        .build()
        .await
        .expect("build");

    let _ = session
        .execute_bash_interactive("echo hello", BashOptions { exclude_from_context: true }, None)
        .await;

    let seen = probe.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the user_bash handler fired exactly once");
    assert_eq!(seen[0].0, "echo hello", "the live command is delivered");
    assert!(seen[0].1, "the !!-prefix excludeFromContext flag is delivered");
    assert_eq!(seen[0].2, fx.cwd.display().to_string(), "the agent cwd is delivered");
}

/// B/user_bash (RPC path): the bare `execute_bash` — the exact method
/// `crates/cyrup-modes/src/rpc.rs`'s `SessionCommand::Bash` arm calls for the JSON-RPC `bash`
/// command — fires NO `user_bash` extension event. Pi's `executeBash` (`agent-session.ts:2582-
/// 2684`) has zero `emitUserBash` emission, and `rpc-mode.ts:550-554`'s `case "bash"` calls
/// `session.executeBash(...)` directly; only the interactive `!`/`!!`-prefix handler
/// (`interactive-mode.ts:5663-5669`) emits the event, proven by
/// `execute_bash_interactive_emits_user_bash_with_live_values` above.
#[tokio::test]
async fn execute_bash_never_emits_user_bash() {
    let fx = fixture();
    let probe: BashProbe = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(UserBashProbe(probe.clone())))
        .build()
        .await
        .expect("build");

    let _ = session
        .execute_bash("echo hello", BashOptions { exclude_from_context: true }, None)
        .await;

    let seen = probe.lock().unwrap().clone();
    assert!(seen.is_empty(), "the RPC-reachable execute_bash must never fire user_bash: {seen:?}");
}

// ============================================ L4 gap #5: session_before_compact typed override ====

/// A native extension subscribed to `session_before_compact` that READS the typed
/// `CompactionPreparation` off the event and returns a custom-summary override (Pi
/// `SessionBeforeCompactResult.compaction`, agent-session.ts:1672-1693). Records the preparation it
/// observed so the test can assert the typed payload actually crossed the seam.
struct CompactionOverrider {
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
}
#[async_trait::async_trait]
impl NativeExtension for CompactionOverrider {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("compaction-overrider")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionBeforeCompact]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionBeforeCompact { preparation, reason, .. } => {
                self.seen.lock().unwrap().push(preparation.clone());
                // Derive the override summary from the REAL preparation so the assertion proves the
                // typed payload was read, not fabricated.
                let first_kept = preparation
                    .get("firstKeptEntryId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                HookOutcome::Mutate(EventPatch::CompactionOverride(serde_json::json!({
                    "summary": format!("ext-summary[{reason}|firstKept={first_kept}]"),
                })))
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// L4 gap #5: an ASSEMBLED manual compaction where a native guest reads the typed
/// `CompactionPreparation` and returns a custom-summary override — the override lands in the appended
/// compaction entry (`fromExtension`) and flows out as the `CompactionResult.summary`, replacing the
/// default model summarization (no summarizer call needed).
#[tokio::test]
async fn compaction_before_compact_override_lands_in_entry() {
    let fx = fixture();
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let faux = Arc::new(FauxProvider::new());
    // Only the two turn responses — the override skips the model summarizer entirely.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .with_native_extension(Arc::new(CompactionOverrider { seen: seen.clone() }))
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let cr = session
        .compact(None)
        .await
        .expect("an aggressive-keep compaction over two turns produces a result");

    // The extension read a REAL preparation: it carries the Pi `CompactionPreparation` fields.
    let observed = seen.lock().unwrap().clone();
    assert_eq!(observed.len(), 1, "the before_compact hook fired exactly once");
    let prep = &observed[0];
    assert!(prep.get("firstKeptEntryId").is_some(), "typed preparation carries firstKeptEntryId: {prep}");
    assert!(prep.get("messagesToSummarize").is_some(), "typed preparation carries messagesToSummarize: {prep}");
    assert!(prep.get("tokensBefore").is_some(), "typed preparation carries tokensBefore: {prep}");

    // The override summary landed in the resulting compaction entry (fromExtension), replacing the
    // default model summary.
    assert!(
        cr.summary.starts_with("ext-summary[manual|firstKept="),
        "the extension override summary lands in the compaction result: {}",
        cr.summary
    );

    // And it is durable in the exported JSONL as a compaction entry.
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(jsonl.contains("ext-summary[manual"), "the override summary is persisted: {jsonl}");
    assert!(jsonl.contains("\"type\":\"compaction\""), "a compaction entry was appended");
}

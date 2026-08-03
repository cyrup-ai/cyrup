//! FULLY-WIRED PROOFS (real OS subprocesses, no mocks of the wired code) for the intercom-companion
//! wiring into cyrup-ext-subagents (reconciliation §4 step 5 items 1–3), each driving the REAL
//! spawn/exec/tool production path against the scripted-NDJSON `cyrup-subagent-fixture` binary:
//!
//! - **(a)** R-SA-P1: the canonical parent-session anchor `CYRUP_SUBAGENT_PARENT_SESSION` is emitted
//!   into a REAL child subprocess's environment set to the launching session's own id — the exact
//!   value the permission companion's child gate reads (`forwarding/mod.rs`) to address the parent's
//!   ask-forwarding inbox. Proven by driving `exec::run_sync` (which calls the single spawn-plan
//!   chokepoint `build_attempt_spawn_plan`) with an explicit `parent_session_id` and reading it back
//!   out of the real child's own stdout `echo_env` tee.
//! - **(b)** R-SA-037/119/120: a child's blocking `contact_supervisor` ask (`need_decision`) fires
//!   the exec detach-trigger arm — `spawn_clarify` surfaces the ask through the REAL `ClarifyChannel`
//!   (pausing the flow) and the attempt is marked `detached`. Proven by scripting the fixture to emit
//!   the blocking-ask NDJSON and asserting the channel received the prompt + `SingleResult.detached`.
//! - **(c)** R-SA-123/124/125: a grouped (parallel) run's result is delivered OUT-OF-BAND through the
//!   REAL `DeliveryChannel`, and on a confirmed delivery the inline tool receipt is REDUCED (the heavy
//!   per-task output dropped). Proven by dispatching the real `subagent` tool `tasks[]` path against a
//!   confirming delivery channel and asserting the reduced receipt + `outOfBandDelivered: true`.
//!
//! Gated on `test-fixtures` (matching the `cyrup-subagent-fixture` `[[bin]]` `required-features`).

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use cyrup_core::{CancelToken, Content, ModelId, Tool, ToolCallId};
use cyrup_ext_subagents::background::RunId;
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;
use cyrup_ext_subagents::tui::intercom::{
    AskLock, ClarifyChannel, ClarifyDispatch, ClarifyRequest, DeliveryChannel, IntercomPayload,
    NoTransportSteerChannel, SteerChannel,
};

/// Serializes every test mutating `CYRUP_SUBAGENT_BINARY`/`CYRUP_SUBAGENT_FIXTURE_SCRIPT` (global).
static ENV_MUTATION_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

fn write_script(dir: &Path, name: &str, script: &serde_json::Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, script.to_string()).expect("write fixture script");
    path
}

/// RAII guard installing the fixture-binary + script env for one test.
struct FixtureEnvGuard;
impl FixtureEnvGuard {
    fn install(script_path: &Path) -> Self {
        let fixture = fixture_binary_path();
        // SAFETY: scoped, mutex-serialized env mutation (Rust 2024 requires `unsafe` for set_var).
        unsafe {
            std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
            std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, script_path);
        }
        Self
    }
}
impl Drop for FixtureEnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `install`.
        unsafe {
            std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
            std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        }
    }
}

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "stop"
        }
    })
    .to_string()
}

fn base_agent_config(model: &str) -> AgentConfig {
    AgentConfig {
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        depth: DepthEnvelope { current_depth: 0, max_depth: 5 },
    }
}

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from(model)],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        skills: None,
        runtime_cwd: None,
        include_progress: None,
        agent_scope: None,
        acceptance: Some(AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![])),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        // SUBA-003: no `subagents.modelScope` policy configured for this fixture.
        model_scope: None,
    }
}

fn read_attempt_tee(child_cwd: &Path) -> String {
    std::fs::read_to_string(child_cwd.join(".cyrup-subagent-scratch").join("attempt-0.jsonl"))
        .unwrap_or_default()
}

// =================================================================================================
// (a) R-SA-P1: the parent-session anchor reaches a REAL child subprocess's environment.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_session_anchor_is_emitted_into_the_real_child_subprocess_env() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    // The fixture echoes back each requested env var it observes as one NDJSON line into its stdout
    // (which run_sync tees to `.cyrup-subagent-scratch/attempt-0.jsonl`). Alongside the parent-session
    // anchor, request the five child-INTERCOM-BRIDGE vars so this proves the SAME production spawn
    // overlay (`build_attempt_spawn_plan`) activates the child intercom bridge in a REAL subprocess.
    let script = serde_json::json!({
        "echo_env": [
            "CYRUP_SUBAGENT_PARENT_SESSION",
            "CYRUP_SUBAGENT_ORCHESTRATOR_TARGET",
            "CYRUP_SUBAGENT_RUN_ID",
            "CYRUP_SUBAGENT_CHILD_AGENT",
            "CYRUP_SUBAGENT_CHILD_INDEX",
            "CYRUP_SUBAGENT_INTERCOM_SESSION_NAME",
        ],
        "steps": [ {"kind": "emit", "line": message_end_line("done")} ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "anchor.json", &script);
    let _env = FixtureEnvGuard::install(&script_path);

    let anchor = "orchestrator-session-2f9a11";
    let agent = base_agent_config("fixture-model"); // persona name = "worker" (see helper)
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // The EXPLICIT anchor the root orchestrator captures from `HostServices::session_id()` at
    // SessionStart and threads through `RunOptions.parent_session_id` (extension.rs) → the spawn env.
    opts.parent_session_id = Some(anchor.to_string());
    // The intercom child-bridge activation the foreground/background populate sites thread in:
    // the orchestrator's own presence target + this run's id + this child's flat index. The spawn
    // overlay MUST fold these into the real child's env so its `IntercomExtension` reads
    // `read_child_orchestrator_metadata() == Some` and registers `contact_supervisor` live.
    opts.orchestrator_intercom_target = Some("subagent-chat-2f9a11ab".to_string());
    opts.run_id = Some(cyrup_ext_subagents::background::RunId::from_token("run-2f9a11"));
    opts.child_index = Some(0);

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "do the thing", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast fixture child");
    assert_eq!(result.exit_code, 0, "clean run: {result:?}");

    let tee = read_attempt_tee(dir.path());
    assert!(
        tee.contains(anchor),
        "the REAL child must have received CYRUP_SUBAGENT_PARENT_SESSION={anchor} in its env \
         (the value the permission child gate reads to address the parent inbox); tee was:\n{tee}",
    );
    assert!(
        tee.contains("CYRUP_SUBAGENT_PARENT_SESSION"),
        "the anchor env var name itself must reach the child: {tee}",
    );
    // The child intercom bridge is LIVE in the real subprocess env: the supervisor target, the run
    // id, the child's persona ("worker") + index, and the child's OWN deterministic presence label
    // (`resolve_subagent_intercom_target("run-2f9a11", "worker", 0)` = `subagent-worker-run-2f9a11-1`)
    // all reached the child — exactly the four required + one label var
    // `read_child_orchestrator_metadata` gates `contact_supervisor` registration on.
    assert!(
        tee.contains("subagent-chat-2f9a11ab"),
        "the child must have received the orchestrator target it addresses contact_supervisor to: {tee}",
    );
    assert!(
        tee.contains("CYRUP_SUBAGENT_ORCHESTRATOR_TARGET")
            && tee.contains("CYRUP_SUBAGENT_RUN_ID")
            && tee.contains("CYRUP_SUBAGENT_CHILD_AGENT")
            && tee.contains("CYRUP_SUBAGENT_CHILD_INDEX")
            && tee.contains("CYRUP_SUBAGENT_INTERCOM_SESSION_NAME"),
        "all four gate-required child-bridge vars + the child's own label var must reach the real \
         child (else its `read_child_orchestrator_metadata` returns None and it never registers \
         contact_supervisor): {tee}",
    );
    assert!(
        tee.contains("subagent-worker-run-2f9a11-1"),
        "the child's OWN presence label must be the deterministic \
         resolve_subagent_intercom_target(run_id, agent, index) string the parent steers: {tee}",
    );
}

// =================================================================================================
// (b) R-SA-037/119/120: a blocking contact_supervisor ask fires spawn_clarify + marks detached.
// =================================================================================================

/// A `ClarifyChannel` that records every ask it receives and answers immediately (the answer would,
/// for the real intercom channel, route back to the still-alive child over the broker).
struct RecordingClarifyChannel {
    asks: Arc<Mutex<Vec<ClarifyRequest>>>,
}
impl ClarifyChannel for RecordingClarifyChannel {
    fn ask(&self, request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        let asks = self.asks.clone();
        Box::pin(async move {
            asks.lock().expect("asks lock").push(request);
            Ok("use postgres".to_string())
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_contact_supervisor_block_fires_clarify_and_marks_the_attempt_detached() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");

    // The child emits a BLOCKING contact_supervisor ask (`need_decision`), then finishes its turn.
    let block_line = serde_json::json!({
        "type": "tool_execution_start",
        "toolCallId": "c1",
        "toolName": "contact_supervisor",
        "args": {"reason": "need_decision", "message": "Which database should I use?"}
    })
    .to_string();
    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": block_line},
            {"kind": "emit", "line": message_end_line("proceeding with the supervisor's answer")}
        ],
        "exit_code": 0
    });
    let script_path = write_script(dir.path(), "clarify.json", &script);
    let _env = FixtureEnvGuard::install(&script_path);

    // The executor's single-slot AskLock backed by a REAL (recording) ClarifyChannel — exactly what
    // `with_channels` wires from the intercom companion's broker channel in production.
    let asks = Arc::new(Mutex::new(Vec::new()));
    let lock = Arc::new(AskLock::new(Arc::new(RecordingClarifyChannel { asks: asks.clone() })));

    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.clarify = Some(ClarifyDispatch {
        lock,
        session_key: "orchestrator-session".to_string(),
        run_id: RunId::from_token("runclarifyb00000"),
        step_index: Some(0),
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        cyrup_ext_subagents::exec::run_sync(&agent, "pick a database", &opts),
    )
    .await
    .expect("run_sync must not hang");

    // R-SA-037: the attempt is marked detached (bypasses acceptance/completion-guard/truncation).
    assert!(result.detached, "the blocking contact_supervisor ask must mark the attempt detached: {result:?}");

    // R-SA-119/120: the ask was surfaced through the REAL ClarifyChannel (the foreground flow paused
    // on it) — wait briefly for the spawned clarify task to record it.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !asks.lock().expect("asks lock").is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "the clarify channel was never reached (no pause/surface)");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let recorded = asks.lock().expect("asks lock");
    assert_eq!(recorded.len(), 1, "exactly one clarify surfaced, got: {:?}", recorded.len());
    assert_eq!(recorded[0].prompt, "Which database should I use?", "the child's ask prompt was surfaced verbatim");
    assert_eq!(recorded[0].step_index, Some(0), "the affected step was carried through");
}

// =================================================================================================
// (c) R-SA-123/124/125: grouped result delivered out-of-band via the real DeliveryChannel + reduced.
// =================================================================================================

/// A `DeliveryChannel` that CONFIRMS receipt (Ok(true)) and records the payload it received — the
/// "a receiver is present" case that authorizes the caller to reduce its inline receipt (R-SA-123).
struct ConfirmingDeliveryChannel {
    received: Arc<Mutex<Vec<IntercomPayload>>>,
}
impl DeliveryChannel for ConfirmingDeliveryChannel {
    fn send(&self, payload: IntercomPayload) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        let received = self.received.clone();
        Box::pin(async move {
            received.lock().expect("received lock").push(payload);
            Ok(true)
        })
    }
}

/// A no-op ClarifyChannel (this proof exercises only the delivery leg).
struct InertClarifyChannel;
impl ClarifyChannel for InertClarifyChannel {
    fn ask(&self, _request: ClarifyRequest) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async { Err("inert".to_string()) })
    }
}

fn write_fixture_persona(cwd: &Path, local_name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{local_name}.md")),
        format!("---\nname: {local_name}\ndescription: fixture persona\nmodel: fixture/model\n---\n\nTest persona.\n"),
    )
    .expect("write persona");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grouped_result_is_delivered_out_of_band_and_the_inline_receipt_is_reduced() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let work_dir = tempfile::tempdir().expect("tempdir");
    write_fixture_persona(work_dir.path(), "worker");

    let cwd_a = work_dir.path().join("task-a");
    let cwd_b = work_dir.path().join("task-b");
    std::fs::create_dir_all(&cwd_a).expect("mkdir a");
    std::fs::create_dir_all(&cwd_b).expect("mkdir b");

    // Each child produces a HEAVY per-task final output ("HEAVY_TASK_OUTPUT_..."), which the reduced
    // receipt MUST drop once delivery is confirmed out-of-band.
    let script = serde_json::json!({
        "steps": [ {"kind": "emit", "line": message_end_line("HEAVY_TASK_OUTPUT_marker_zzz")} ],
        "exit_code": 0
    });
    let script_path = write_script(work_dir.path(), "delivery.json", &script);
    let _env = FixtureEnvGuard::install(&script_path);

    // Build the extension with the REAL (confirming) DeliveryChannel — exactly what `with_channels`
    // threads from the intercom companion in production.
    let received = Arc::new(Mutex::new(Vec::new()));
    let delivery: Arc<dyn DeliveryChannel> = Arc::new(ConfirmingDeliveryChannel { received: received.clone() });
    let clarify: Arc<dyn ClarifyChannel> = Arc::new(InertClarifyChannel);
    // This proof exercises only the delivery leg; the steer leg (R-SA-086 live-child follow-up) is
    // the no-transport default, which never fires in this out-of-band-delivery scenario.
    let steer: Arc<dyn SteerChannel> = Arc::new(NoTransportSteerChannel);
    let ext = SubagentsExtension::with_channels(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
        delivery,
        clarify,
        steer,
    );

    let result = ext
        .subagent_tool()
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({
                "tasks": [
                    { "agent": "worker", "task": "ALPHA", "cwd": cwd_a.to_string_lossy() },
                    { "agent": "worker", "task": "BETA", "cwd": cwd_b.to_string_lossy() },
                ]
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("tool execute must succeed");

    let text: String = result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    // R-SA-124: the FULL grouped result reached the out-of-band channel (with the heavy output).
    let got = received.lock().expect("received lock");
    assert_eq!(got.len(), 1, "the grouped result was delivered out-of-band exactly once");
    assert!(
        got[0].outputs.iter().any(|o| o.contains("HEAVY_TASK_OUTPUT_marker_zzz")),
        "the out-of-band payload carries the full per-task output: {:?}",
        got[0].outputs,
    );

    // R-SA-123: the inline receipt is REDUCED — the heavy per-task output is dropped, replaced by
    // pi's exact `formatSubagentResultReceipt` wording (result-intercom.ts:334-364): "Delivered
    // <mode> subagent results via intercom." + a real "Run: <id>" (never a throwaway) + a child
    // status count line.
    assert!(
        text.contains("Delivered parallel subagent results via intercom.")
            && text.contains("Children: 2 completed"),
        "the inline receipt must be the reduced, pi-format out-of-band receipt: {text}",
    );
    assert!(
        !text.contains("HEAVY_TASK_OUTPUT_marker_zzz"),
        "R-SA-123: the heavy duplicated output MUST NOT remain inline once delivered out-of-band: {text}",
    );

    // The structured details flag the out-of-band delivery for the LLM/host.
    let details = result.details.expect("details present");
    assert_eq!(details.get("outOfBandDelivered").and_then(serde_json::Value::as_bool), Some(true));
}

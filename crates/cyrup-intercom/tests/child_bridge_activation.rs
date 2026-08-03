//! FULLY-WIRED PRODUCTION-ACTIVATION PROOF (the claim-6 hard-blocker close): a child subagent
//! spawned through the REAL production spawn path — `cyrup_ext_subagents::exec::run_sync` →
//! `build_attempt_spawn_plan` → `.envs(spec.env_overlay)` — activates its intercom bridge with
//! NOTHING hand-injected by this test: the six `CYRUP_SUBAGENT_*` child-bridge env vars are written
//! ONLY by the production overlay. The spawned child then:
//!
//! 1. reads that env via the production `read_child_orchestrator_metadata()` gate (else it exits
//!    non-zero → this test fails), and
//! 2. registers on the REAL broker under its deterministic label
//!    (`resolve_subagent_intercom_target(run_id, agent, index)`), and
//! 3. sends a `contact_supervisor`-shaped ask to its supervisor over the broker + receives the
//!    supervisor's reply — the full round trip, proven at BOTH ends (the supervisor observes the ask;
//!    the child echoes the reply into `run_sync`'s attempt log).
//!
//! This is the test whose ABSENCE the adversarial audit flagged as leaving the seam "production
//! inert": `companions_wiring_proof.rs` proves the overlay DELIVERS the env to a scripted fixture (no
//! broker registration), and `broker_roundtrip.rs` proves a round trip with a HAND-CONSTRUCTED child
//! registration. Only this test joins the two halves through the genuine spawn path.
//!
//! Gated on `test-fixtures` (matching the `cyrup-intercom-child-fixture` `[[bin]]` `required-features`).

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex as AsyncMutex;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::background::RunId;
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{Message, SessionInfo, SessionRegistration, now_ms};
use cyrup_intercom::transport::spawn::wait_for_broker;

/// Serializes the two globally-scoped env mutations (`CYRUP_SUBAGENT_BINARY` +
/// `CYRUP_CODING_AGENT_DIR`) this test performs so a future sibling test in this binary never races.
static ENV_MUTATION_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

fn child_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-child-fixture"))
}

/// RAII guard installing, for one test, the two process-global env vars the production `run_sync`
/// child inherits: the fixture binary (`CYRUP_SUBAGENT_BINARY`, `resolve_spawn_command`'s tier-1
/// override) and the broker's agent dir (`CYRUP_CODING_AGENT_DIR`, how the child discovers the
/// socket). NEITHER is a child-bridge var — those are set only by the production spawn overlay.
struct SpawnEnvGuard;
impl SpawnEnvGuard {
    fn install(child_binary: &Path, agent_dir: &Path) -> Self {
        // SAFETY: scoped, mutex-serialized env mutation (Rust 2024 requires `unsafe` for set_var).
        unsafe {
            std::env::set_var("CYRUP_SUBAGENT_BINARY", child_binary);
            std::env::set_var("CYRUP_CODING_AGENT_DIR", agent_dir);
        }
        Self
    }
}
impl Drop for SpawnEnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `install`.
        unsafe {
            std::env::remove_var("CYRUP_SUBAGENT_BINARY");
            std::env::remove_var("CYRUP_CODING_AGENT_DIR");
        }
    }
}

fn registration(name: &str) -> SessionRegistration {
    SessionRegistration {
        name: Some(name.to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id(),
        started_at: now_ms(),
        last_activity: now_ms(),
        status: None,
    }
}

async fn next_message(
    rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>,
) -> (SessionInfo, Message) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { from, message })) => return (from, message),
            Ok(Ok(_other)) => continue,
            Ok(Err(e)) => panic!("event channel error: {e}"),
            Err(_) => panic!("timed out waiting for the child's ask over the broker"),
        }
    }
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
        // SUBA-003: no `subagents.modelScope` policy in this fixture — enforcement off.
        model_scope: None,
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
    }
}

fn read_attempt_tee(child_cwd: &Path) -> String {
    std::fs::read_to_string(child_cwd.join(".cyrup-subagent-scratch").join("attempt-0.jsonl"))
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_spawned_child_registers_on_the_broker_and_round_trips_with_its_supervisor() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    // A real broker as a genuine child process, pointed at a temp agent dir.
    let agent_dir = tempfile::tempdir().expect("agent tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");
    let broker_bin = PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"));
    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess");
    wait_for_broker(&socket_path, Duration::from_secs(5))
        .await
        .expect("broker becomes health-connectable");

    // The supervisor registers under the SAME presence NAME the production overlay will hand the
    // child as `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET`, so the child's ask resolves to it by name at the
    // broker (the exact address a real top-level orchestrator registers under).
    let orchestrator_target = "subagent-chat-supervisor01";
    let supervisor = Arc::new(
        IntercomClient::connect(
            &socket_path,
            registration(orchestrator_target),
            Some("supervisor-session".to_string()),
        )
        .await
        .expect("supervisor registers"),
    );
    let mut supervisor_events = supervisor.subscribe();

    // A concurrent task that RECEIVES the child's ask and replies with a unique marker (the reply
    // would, in production, route back to the still-alive child over the broker — which is exactly
    // what we prove reaches the child below).
    let reply_marker = "USE_POSTGRES_9f3a7c";
    let supervisor_task = {
        let supervisor = supervisor.clone();
        tokio::spawn(async move {
            let (from, ask) = next_message(&mut supervisor_events).await;
            supervisor
                .send(
                    &from.id,
                    SendOptions {
                        text: reply_marker.to_string(),
                        reply_to: Some(ask.id.clone()),
                        ..Default::default()
                    },
                )
                .await
                .expect("supervisor reply routes through the broker");
            (from, ask)
        })
    };

    // Drive the REAL production spawn path. The child binary is our self-registering fixture; this
    // test writes NONE of the six `CYRUP_SUBAGENT_*` bridge vars — `build_attempt_spawn_plan` does,
    // gated on `orchestrator_intercom_target` + `run_id` (both `Some` below) + a non-empty agent name.
    let work = tempfile::tempdir().expect("work tempdir");
    let _env = SpawnEnvGuard::install(&child_fixture_path(), agent_dir.path());

    let agent = base_agent_config("fixture-model"); // persona name = "worker"
    let mut opts = base_run_options(work.path(), "fixture-model");
    opts.orchestrator_intercom_target = Some(orchestrator_target.to_string());
    opts.run_id = Some(RunId::from_token("run-bridge01"));
    opts.child_index = Some(0);

    let result = tokio::time::timeout(
        Duration::from_secs(40),
        cyrup_ext_subagents::exec::run_sync(&agent, "coordinate with the supervisor", &opts),
    )
    .await
    .expect("run_sync must not hang against the fast child fixture");

    let (from, ask) = tokio::time::timeout(Duration::from_secs(10), supervisor_task)
        .await
        .expect("supervisor task must complete")
        .expect("supervisor task join");

    // (1) The production-spawned child REGISTERED on the broker under its deterministic label
    // `resolve_subagent_intercom_target("run-bridge01", "worker", 0)` = `subagent-worker-run-bridge01-1`
    // — proving `read_child_orchestrator_metadata()` returned Some off the production-set env.
    assert_eq!(
        from.name.as_deref(),
        Some("subagent-worker-run-bridge01-1"),
        "the child must register under the deterministic resolve_subagent_intercom_target label; \
         got {from:?}",
    );
    // (2) It addressed THIS supervisor with a reply-expecting ask carrying its run/agent identity.
    assert_eq!(ask.expects_reply, Some(true), "the child's ask must record an ask edge: {ask:?}");
    assert!(
        ask.content.text.contains("run-bridge01") && ask.content.text.contains("worker"),
        "the child's ask body must carry its production-set run/agent identity: {}",
        ask.content.text,
    );
    // (3) The supervisor's reply REACHED the real child over the broker: it exited cleanly (metadata
    // was Some + the round trip closed) and echoed the reply into run_sync's attempt log.
    assert_eq!(
        result.exit_code, 0,
        "a non-zero child exit means the bridge was inert or the round trip failed: {result:?}",
    );
    let tee = read_attempt_tee(work.path());
    assert!(
        tee.contains("SUPERVISOR_REPLY") && tee.contains(reply_marker),
        "the supervisor's reply must have reached the real spawned child over the broker; tee:\n{tee}",
    );

    supervisor.disconnect();
    let _ = broker.kill().await;
}

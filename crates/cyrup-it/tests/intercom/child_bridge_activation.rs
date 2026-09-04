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
//! MIGRATION — the `#![cfg(feature = "test-fixtures")]` this file carried in
//! `crates/cyrup-intercom/tests/` is DELIBERATELY GONE, and dropping it is what keeps the test
//! running rather than what disables it. That gate existed for exactly one reason: inside
//! cyrup-intercom's own test crate, `env!("CARGO_BIN_EXE_cyrup-intercom-child-fixture")` is defined
//! only when that crate's `test-fixtures` feature builds the `[[bin]]`, so without the cfg the file
//! failed to COMPILE with the feature off. Here the path comes from
//! `support::bins::intercom_child_fixture()`, which `build.rs` resolves unconditionally (it passes
//! `--features test-fixtures` to the nested `cargo build -p cyrup-intercom`), so the compile-time
//! dependency is gone. Re-adding a `test-fixtures` feature to `cyrup-it` and gating on it would
//! make `cargo test -p cyrup-it --features it` SKIP this test in silence — the invisible-skip
//! failure mode the whole `required-features` gate exists to prevent.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{CancelToken, ModelId};
use cyrup_ext_subagents::background::RunId;
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;

use crate::common::registration;
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{Message, SessionInfo};
use cyrup_intercom::transport::spawn::wait_for_broker;

fn child_fixture_path() -> PathBuf {
    crate::support::bins::intercom_child_fixture()
}

/// The two values the production `run_sync` child needs, supplied per-run instead of exported.
///
/// The fixture binary rides on `RunOptions::spawn_command`; the broker's agent dir is a variable
/// the CHILD reads to find the socket, so it goes on `RunOptions::child_env` — R2 tier 2, set on
/// the child's `Command` rather than on this process. NEITHER is a child-bridge var; those are set
/// only by the production spawn overlay.
fn spawn_env(child_binary: &Path, agent_dir: &Path) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([(
        "CYRUP_CODING_AGENT_DIR".to_string(),
        agent_dir.display().to_string(),
    )])
    .into_iter()
    .chain(std::iter::once((
        "CYRUP_SUBAGENT_BINARY".to_string(),
        child_binary.display().to_string(),
    )))
    .collect()
}

async fn next_message(
    rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>,
) -> (SessionInfo, Message) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { from, message })) => return (from, *message),
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
        model_provider: None,
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
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
        // Added when the agent-definition fields landed (G95 `memory:`, G89 `toolBudget:`); this
        // fixture declares neither, which is the same as an agent file omitting them.
        memory: None,
        tool_budget: None,
        runner: None,          // SUBA-074: the native child, as before
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
        exclude_tools: Vec::new(), // SUBA-092: no exclusions (this literal predates the field)
        allow_nested_subagents: None,
    }
}

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        turn_budget: None,
        permission_rules: None, // SUBA-073: no policy — the pre-field behaviour
        // SUBA-078: this fixture exercises no reasoning ceiling — `None` is "no ceiling
        // configured, so the bound is off", matching `runner_main.rs`'s own hop-2 default.
        thinking_ceiling: None,
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        enforce_hard_turn_limit: false,
        // SUBA-003: no `subagents.modelScope` policy in this fixture — enforcement off.
        model_scope: None,
        // Added with G90's steer inbox. `None` is upstream's foreground shape — only a background
        // step gets a steer inbox (`subagent-runner.ts` step dirs), so this bridge fixture has none.
        steer_inbox_dir: None,
        // SUBA-049: the RETURN half of G90's steer channel. Both paths exist only under a background
        // run directory; a foreground fixture like this one has none. Load-bearing:
        // `build_attempt_spawn_plan` gates both env keys on presence (exec/mod.rs:2227-2250), so
        // `None` keeps the child's env overlay byte-identical to a real foreground child's.
        steer_ack_dir: None,
        steer_capability_path: None,
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        // SUBA-054: `None` is upstream's `false` — no `reads` instruction at all.
        reads: None,
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
        acceptance: Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            vec![],
        )),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        control_config: None,
        on_control_event: None,
        // Added with G80's verify-command memoization. `None` is the pre-G80 behaviour (no
        // artifacts root configured => no memoization), which is what this bridge fixture wants:
        // its subject is the intercom child bridge, not the acceptance gate.
        artifacts_dir: None,
    }
}

/// The per-attempt raw-stdout tee `exec::run_sync` wrote for the child that ran in `child_cwd` —
/// `<attempt_scratch_dir(child_cwd)>/attempt-0.jsonl` (SUBA-072: under the crate's run-scratch
/// root, keyed by cwd, never under the project tree).
fn read_attempt_tee(child_cwd: &Path) -> String {
    std::fs::read_to_string(
        cyrup_ext_subagents::background::attempt_scratch_dir(child_cwd).join("attempt-0.jsonl"),
    )
    .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_spawned_child_registers_on_the_broker_and_round_trips_with_its_supervisor() {
    // A real broker as a genuine child process, pointed at a temp agent dir.
    let agent_dir = tempfile::tempdir().expect("agent tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");
    let broker_bin = crate::support::bins::intercom_broker();
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

    let agent = base_agent_config("fixture-model"); // persona name = "worker"
    let mut opts = base_run_options(work.path(), "fixture-model");
    opts.spawn_command = Some(SpawnCommand {
        binary: child_fixture_path(),
        base_args: Vec::new(),
    });
    opts.child_env = spawn_env(&child_fixture_path(), agent_dir.path());
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
    assert_eq!(
        ask.expects_reply,
        Some(true),
        "the child's ask must record an ask edge: {ask:?}"
    );
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

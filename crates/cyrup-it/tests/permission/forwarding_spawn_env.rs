//! PERM-001 — the PRODUCTION-SPAWN proof that a subagent child forwards its permission asks to the
//! parent session.
//!
//! `tests/forwarding_subprocess.rs` already proves the spool TRANSPORT works, but it arms the child
//! by hand (`cmd.env("CYRUP_SUBAGENT_CHILD", "1")`). That is exactly what production never did: the
//! gate read `CYRUP_SUBAGENT_CHILD`, and no production spawn path wrote it, so every real subagent
//! child looked like a top-level session, got the LOCAL ask channel, found no TTY behind it and
//! fail-CLOSED every `ask`-tier tool call instead of asking the parent's human.
//!
//! So this file arms the child from the REAL spawn planner and nothing else: it calls
//! `cyrup_ext_subagents::exec::build_attempt_spawn_plan` — the one non-test site that builds a
//! `ChildSpawnSpec` for a foreground subagent attempt — and applies that plan's `env_overlay`
//! verbatim to the child process, exactly as `SpawnedChild::spawn` does in production. No literal
//! `"CYRUP_SUBAGENT_CHILD"` (nor any other spawn-env key) is typed anywhere below; if the spawn
//! planner stops marking its children, these tests go red with it.
//!
//! The channel under proof is the FILESYSTEM SPOOL at
//! `<agentDir>/sessions/permission-forwarding/sessions/<urlencode(sessionId)>/{requests,responses}`
//! (pi `permission-forwarding.ts:74-127`) — **not** intercom, which is a separate Unix-socket
//! subsystem and is not involved anywhere in this file.
//!
//! - [`spawn_env_alone_carries_a_child_ask_into_the_parent_spool`] — no responder at all: the test
//!   reads the child's REQUEST JSON off the parent's inbox directory and asserts its addressing,
//!   then watches the child fail-CLOSE. This is the file-level proof the ask crossed.
//! - [`spawn_env_alone_lets_the_parents_human_answer_a_child_ask`] — the full round trip: the REAL
//!   `spawn_forwarding_watcher` + a scripted human ALLOW, and the child's tool then proceeds.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, ModelId, ToolCallId};
use cyrup_ext::{
    ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, HumanInteractionLock, InitApi,
};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{build_attempt_spawn_plan, AgentConfig, RunModelOverride, RunOptions};
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;
use cyrup_permission_system::forwarding::{forwarding_location, read_request};
use cyrup_permission_system::{permission_extension_for_env, spawn_forwarding_watcher, ExtensionConfig};

// The scripted parent-side host, the ASK policy and the child reaper are shared with
// `forwarding_subprocess.rs` — see `forwarding_common.rs`. `spawn_child` is NOT shared: this file's
// whole point is that its child env comes from the production spawn planner.
use crate::forwarding_common::{
    wait_child, write_policy, ScriptedHost, EXIT_ALLOWED, EXIT_BLOCKED, EXIT_UNEXPECTED,
};

// ---- env contract between the parent test and the re-exec'd child role ----
const CHILD_ROLE_ENV: &str = "PERM_SPAWNENV_CHILD_ROLE";
const CHILD_AGENT_DIR_ENV: &str = "PERM_SPAWNENV_AGENT_DIR";
const CHILD_SENTINEL_ENV: &str = "PERM_SPAWNENV_SENTINEL";
// MIGRATION REWRITE: libtest names a test by its FULL path, and this file is now a `mod` of the
// `permission` binary rather than an integration binary of its own. Left bare, `--exact` would match
// nothing, the child would run zero tests and exit 0, and `spawn_env_alone_carries_a_child_ask_into_
// the_parent_spool` would read that as EXIT_ALLOWED instead of the EXIT_BLOCKED it asserts.
const CHILD_TEST_NAME: &str = "forwarding_spawn_env::spawn_env_child_role_entry";

/// The agent name the production overlay threads as `AGENT_NAME_ENV_VAR`; asserted back out of the
/// request JSON so the test proves the PARENT sees WHICH persona is asking.
const CHILD_AGENT_NAME: &str = "reviewer";

/// A scripted [`HostServices`] whose only override is the tool registry — see the identical helper
/// in `tests/forwarding_subprocess.rs`. `AgentSession::build` calls `set_host_services` on every
/// native extension before any event reaches it; without it the registry gate blocks `bash` as
/// unregistered before the ask-forwarding logic under test ever runs.
struct ChildRegistryServices;
impl HostServices for ChildRegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["bash".to_string()])
    }
}

// =================================================================================================
// The CHILD role — a REAL separate process running the production gate entry point.
// =================================================================================================

/// Inert in a normal run (env unset → returns). Re-exec'd by the parent tests below with the
/// PRODUCTION child env applied; it then builds the gate through `permission_extension_for_env` and
/// drives one `ask`-tier `bash` call through the registered `before_tool_call` hook.
#[test]
fn spawn_env_child_role_entry() {
    if std::env::var(CHILD_ROLE_ENV).as_deref() != Ok("1") {
        return; // normal test run: not the child.
    }
    let agent_dir =
        PathBuf::from(std::env::var(CHILD_AGENT_DIR_ENV).expect("child needs the agent dir"));
    let sentinel = std::env::var(CHILD_SENTINEL_ENV).expect("child needs the sentinel path");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("child tokio runtime");

    let code = runtime.block_on(async move {
        let Some(ext) = permission_extension_for_env(agent_dir.clone(), agent_dir.clone()) else {
            return EXIT_UNEXPECTED;
        };
        let mut api = InitApi::new();
        if ext.init(&mut api).await.is_err() {
            return EXIT_UNEXPECTED;
        }
        ext.set_host_services(Arc::new(ChildRegistryServices));
        // has_ui = false: the exact shape a re-exec'd `--print --mode json` subagent runs under.
        let ctx = HostCtx::event(ExtMode::Print, false, agent_dir.clone());
        let event = HostEvent::ToolCall {
            call_id: ToolCallId::from("spawn-env-call-1"),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo hi" }),
        };
        match ext.on_event(&event, &ctx).await {
            HookOutcome::Noop => {
                let _ = std::fs::write(&sentinel, b"EXECUTED");
                EXIT_ALLOWED
            }
            HookOutcome::Block { .. } => EXIT_BLOCKED,
            _ => EXIT_UNEXPECTED,
        }
    });
    std::process::exit(code);
}

// =================================================================================================
// The PARENT role — this test process.
// =================================================================================================

/// Build the env overlay for a subagent child THROUGH THE PRODUCTION PLANNER
/// (`exec::build_attempt_spawn_plan`), for an agent named [`CHILD_AGENT_NAME`] delegated from a
/// session whose id is `parent_id`.
///
/// Nothing here hand-writes a spawn-env key: the child-role marker, the parent-session anchor and
/// the persona name all come out of the same assembly a real `subagent` tool call performs.
fn production_child_env(cwd: &Path, parent_id: &str) -> std::collections::HashMap<String, String> {
    let agent = AgentConfig {
        name: CHILD_AGENT_NAME.to_string(),
        model: Some(ModelId::from("m1")),
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
        // G95 `memory:` / G89 `toolBudget:` — this fixture declares neither.
        memory: None,
        tool_budget: None,
    };
    let opts = RunOptions {
        turn_budget: None,
        enforce_hard_turn_limit: false,
        // G90's steer inbox: `None` is the foreground shape.
        steer_inbox_dir: None,
        // SUBA-003: no `subagents.modelScope` policy in this fixture — enforcement off.
        model_scope: None,
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        // SUBA-054: `None` is upstream's `false` — no `reads` instruction at all.
        reads: None,
        structured_output_schema: None,
        model_override: RunModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from("m1")],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        skills: None,
        runtime_cwd: None,
        include_progress: None,
        agent_scope: None,
        acceptance: None,
        fork_context: ForkContext::fresh(),
        live_events: None,
        // The launching session's own id — the anchor a child addresses its forwarded asks at.
        parent_session_id: Some(parent_id.to_string()),
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
    };
    let plan = build_attempt_spawn_plan(
        &agent,
        &ModelId::from("m1"),
        "review the diff",
        &opts,
        DepthEnvelope { current_depth: 1, max_depth: 5 },
        cwd,
        // SUBA-S01: no `outputSchema` declared here — this test asserts the PERMISSION
        // forwarding env, and a structured-output runtime would add two unrelated vars to the
        // overlay it inspects.
        None,
    )
    .expect("the production spawn planner must build a plan");
    plan.spec.env_overlay
}

/// Spawn the child role, applying the PRODUCTION spawn overlay to it the way
/// `SpawnedChild::spawn` does — over the inherited environment, no `env_clear`.
fn spawn_child(agent_dir: &Path, parent_id: &str, sentinel: &Path, child_wait_ms: u64) -> std::process::Child {
    let exe = std::env::current_exe().expect("current test exe");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", CHILD_TEST_NAME, "--nocapture", "--test-threads=1"]);
    cmd.env(CHILD_ROLE_ENV, "1");
    cmd.env(CHILD_AGENT_DIR_ENV, agent_dir);
    cmd.env(CHILD_SENTINEL_ENV, sentinel);

    for (key, value) in production_child_env(agent_dir, parent_id) {
        cmd.env(key, value);
    }

    // Bound the child's blocking wait so a wiring bug fails fast instead of hanging on the 10-minute
    // production default. (Not a spawn-env key — an ops override this crate owns.)
    cmd.env("CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS", child_wait_ms.to_string());
    cmd.spawn().expect("spawn child role subprocess")
}

fn unique_parent_id() -> String {
    format!(
        "parent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    )
}

/// Poll the PARENT's spool inbox for the child's request file and return its parsed contents.
async fn await_spooled_request(
    requests_dir: &Path,
    within: Duration,
) -> Option<cyrup_permission_system::ForwardedPermissionRequest> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Ok(entries) = std::fs::read_dir(requests_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json")
                    && let Some(request) = read_request(&path)
                {
                    return Some(request);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

// =================================================================================================
// The proofs
// =================================================================================================

/// PERM-001 core: with ONLY the production spawn env applied, an `ask` fired inside the child lands
/// as a request FILE in the PARENT's forwarding spool.
///
/// No watcher runs here, so nothing can consume the request before it is read: the assertion is on
/// the on-disk artifact itself, addressed to the parent's inbox and naming the asking persona. Then
/// the child fail-CLOSES (nobody answered), which is the correct pi behavior for an unanswered
/// forward and confirms the child was genuinely BLOCKED on the parent, not resolving locally.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_env_alone_carries_a_child_ask_into_the_parent_spool() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    write_policy(agent_dir.path());
    let parent_id = unique_parent_id();
    let sentinel = agent_dir.path().join("child-executed.sentinel");

    // The PARENT's own inbox, resolved the way the parent watcher resolves it: from the parent's
    // session id. The child must independently arrive at this same directory from the anchor env.
    let location = forwarding_location(agent_dir.path(), &parent_id).expect("parent spool location");

    // PERM-022: the child's ask must still be SPOOLED when the 15 s poll below gives up, so its
    // own bound has to outlive that window by construction — 8 s did not, and the test could pass
    // or fail on scheduling. 20 s matches the sibling at the allow test below.
    let child = spawn_child(agent_dir.path(), &parent_id, &sentinel, 20_000);
    let request = await_spooled_request(&location.requests_dir, Duration::from_secs(15)).await;

    let request = request.unwrap_or_else(|| {
        panic!(
            "the child's ask never reached the parent's forwarding spool at {} — a subagent child \
             spawned with the production env must forward its ask, not resolve it locally",
            location.requests_dir.display()
        )
    });
    assert_eq!(
        request.target_session_id, parent_id,
        "the forwarded request must be addressed at the PARENT session anchor"
    );
    assert_eq!(
        request.requester_agent_name, CHILD_AGENT_NAME,
        "the parent must learn WHICH persona is asking (the spawn overlay's agent-name env)"
    );
    assert!(
        request.message.contains("bash"),
        "the forwarded prompt must describe the child's actual tool call, got: {}",
        request.message
    );
    assert!(!request.response_nonce.is_empty(), "the request must carry its response-binding nonce");

    let code = wait_child(child, Duration::from_secs(30)).await;
    assert_eq!(
        code,
        Some(EXIT_BLOCKED),
        "with nobody answering the forward, the child must fail CLOSED (it was waiting on the parent)"
    );
    assert!(!sentinel.exists(), "the child's tool must not have run");
}

/// The round trip: the same production spawn env, plus the REAL parent watcher and a scripted human
/// who allows — the child's tool then proceeds, so the parent's decision genuinely governed the
/// child's tool call across the process boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_env_alone_lets_the_parents_human_answer_a_child_ask() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    write_policy(agent_dir.path());
    let parent_id = unique_parent_id();
    let sentinel = agent_dir.path().join("child-executed.sentinel");

    let selects = Arc::new(AtomicUsize::new(0));
    let services: Arc<dyn HostServices> = Arc::new(ScriptedHost {
        session_id: parent_id.clone(),
        answer: "Allow Once".to_string(),
        selects: selects.clone(),
        lock: Arc::new(HumanInteractionLock::new()),
    });
    let watcher = spawn_forwarding_watcher(
        agent_dir.path().to_path_buf(),
        services,
        Arc::new(Mutex::new(ExtensionConfig::default())),
        // PERM-008: the watcher audits the forwarded request lifecycle
        // (`forwarded_permission.request_created` / `.approved` / `.response_timed_out`), so it
        // needs a trail. `detached` writes under `<agent_dir>/logs` with a default config.
        Arc::new(cyrup_permission_system::AuditTrail::detached(
            agent_dir.path().join("logs"),
        )),
        // These fixtures script a live human, so a UI IS present — the `has_ui` guard must not
        // fail the forwarded ask closed.
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    );

    let child = spawn_child(agent_dir.path(), &parent_id, &sentinel, 20_000);
    let code = wait_child(child, Duration::from_secs(40)).await;
    watcher.abort();

    assert_eq!(
        code,
        Some(EXIT_ALLOWED),
        "the parent human's ALLOW must reach the child spawned with the production env and let its \
         tool proceed"
    );
    assert!(sentinel.exists(), "the child's tool must have run after the cross-process allow");
    assert!(
        selects.load(Ordering::SeqCst) >= 1,
        "the PARENT must have surfaced a prompt written by the CHILD process (boundary crossed)"
    );
}

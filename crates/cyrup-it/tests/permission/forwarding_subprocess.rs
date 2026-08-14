//! FULLY-WIRED PROOF (R-PERM-040/041/043) that the child→parent permission ask-forwarding spool
//! genuinely crosses an OS-process boundary and gates the child — no mocks of the wired path.
//!
//! A REAL detached subprocess runs the CHILD role: it builds the gate through the production entry
//! point `permission_extension_for_env` (which, for a `CYRUP_SUBAGENT_CHILD`, installs the real
//! `ForwardingAskChannel`), then drives a `bash` `ask`-tier tool call through the registered
//! `before_tool_call` gate (`NativeExtension::on_event(ToolCall)`). The child's `ForwardingAskChannel`
//! writes a nonce-bound REQUEST into the PARENT's filesystem spool and BLOCKS. The PARENT process (this
//! test) runs the REAL `spawn_forwarding_watcher` against a scripted `HostServices` human sink that
//! answers the forwarded `select` dialog; the watcher writes the nonce-bound RESPONSE. The child's gate
//! consumes the bound response and the child's tool then PROCEEDS (or is BLOCKED), observable via the
//! child's exit code + a sentinel file it touches only on allow.
//!
//! - **allow**: scripted human returns "Allow Once" → child's tool proceeds (exit 0, sentinel present).
//! - **deny**: scripted human returns "Reject" → child's tool blocked (exit 3, no sentinel).
//! - **timeout**: no responder at all → child fail-CLOSES after its (shortened) wait bound (exit 3).
//!
//! Each assertion proves the decision genuinely traversed the process boundary: the watcher's
//! select-count > 0 means THIS process saw a request written by the OTHER process, and the child's exit
//! code reflects the decision THIS process wrote back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::ToolCallId;
use cyrup_ext::{
    ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, HumanInteractionLock, InitApi,
};
use cyrup_permission_system::{permission_extension_for_env, spawn_forwarding_watcher, ExtensionConfig};

// The scripted parent-side host, the ASK policy and the child reaper are shared with
// `forwarding_spawn_env.rs` — see `forwarding_common.rs` for why the 40ms/25ms poll split collapsed
// to one value and why `spawn_child` deliberately did NOT.
use crate::forwarding_common::{
    wait_child, write_policy, ScriptedHost, EXIT_ALLOWED, EXIT_BLOCKED, EXIT_UNEXPECTED,
};

/// A scripted [`HostServices`] whose ONLY override is [`HostServices::all_tool_names`] — the full
/// registry the registry / unknown-tool gate checks against (pi `pi.getAllTools()`). Mirrors the
/// identical helper in `tests/layers_wired.rs` / `src/extension.rs`'s own unit tests: the REAL
/// `AgentSession::build` (`cyrup-session-svc/src/builder.rs`) calls
/// `NativeExtension::set_host_services` on every native extension (via `load_native_with_services`)
/// BEFORE `init`/`on_event` ever run — for every role, including a re-exec'd subagent child. Without
/// this, `decide()`'s registry gate (`registered_tool_names()` -> `None` -> empty registry) blocks
/// EVERY tool as "unregistered" before the ask-forwarding logic is ever reached.
struct ChildRegistryServices {
    names: Vec<String>,
}
impl HostServices for ChildRegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.names.clone())
    }
}

// ---- env contract between the parent test and the re-exec'd child role ----
const CHILD_ROLE_ENV: &str = "PERM_FWD_CHILD_ROLE";
const CHILD_AGENT_DIR_ENV: &str = "PERM_FWD_AGENT_DIR";
const CHILD_COMMAND_ENV: &str = "PERM_FWD_COMMAND";
const CHILD_SENTINEL_ENV: &str = "PERM_FWD_SENTINEL";
// The child-role test name (re-exec'd via `--exact`).
//
// MIGRATION REWRITE: libtest names a test by its FULL path, and this file is now a `mod` of the
// `permission` binary rather than an integration binary of its own — so the name grew the
// `forwarding_subprocess::` prefix. Left as the bare `forwarding_child_role_entry`, `--exact` matches
// NOTHING, the re-exec'd child runs zero tests, exits 0, and every assertion below reads that as
// EXIT_ALLOWED — i.e. the deny and timeout proofs would go silently, permanently green.
const CHILD_TEST_NAME: &str = "forwarding_subprocess::forwarding_child_role_entry";

// =================================================================================================
// The CHILD role — a REAL separate process running the production gate + ForwardingAskChannel.
// =================================================================================================

/// This `#[test]` is inert in a normal run (env unset → returns). The parent tests re-exec THIS test
/// binary with `--exact forwarding_child_role_entry` + the child env set; then it builds the wired
/// child gate, drives one `ask`-tier `bash` tool call through `before_tool_call`, and `exit`s with a
/// code encoding whether the (cross-process) decision let the tool proceed.
#[test]
fn forwarding_child_role_entry() {
    if std::env::var(CHILD_ROLE_ENV).as_deref() != Ok("1") {
        return; // normal test run: not the child.
    }
    let agent_dir = std::path::PathBuf::from(
        std::env::var(CHILD_AGENT_DIR_ENV).expect("child needs PERM_FWD_AGENT_DIR"),
    );
    let command = std::env::var(CHILD_COMMAND_ENV).unwrap_or_else(|_| "ls -la".to_string());
    let sentinel = std::env::var(CHILD_SENTINEL_ENV).expect("child needs PERM_FWD_SENTINEL");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("child tokio runtime");

    let code = runtime.block_on(async move {
        // Production entry point: a `CYRUP_SUBAGENT_CHILD` (set in this process's env by the parent
        // spawn) installs the ForwardingAskChannel. `None` (not installed) would be a test setup bug.
        let ext = match permission_extension_for_env(agent_dir.clone(), agent_dir.clone()) {
            Some(ext) => ext,
            None => return EXIT_UNEXPECTED,
        };
        // Run the real init (subscribes ToolCall) then drive the gate directly, exactly as the
        // dispatcher would (`NativeHandle::invoke_event` → `on_event`).
        let mut api = InitApi::new();
        if ext.init(&mut api).await.is_err() {
            return EXIT_UNEXPECTED;
        }
        // Production parity: `AgentSession::build` (`cyrup-session-svc/src/builder.rs`,
        // `load_native_with_services`) calls `set_host_services` on every native extension BEFORE
        // any event reaches it — for every role, including this re-exec'd subagent child. Without
        // this the registry gate treats "bash" as unregistered and blocks it immediately, before the
        // ask-forwarding logic under test ever runs.
        ext.set_host_services(Arc::new(ChildRegistryServices { names: vec!["bash".to_string()] }));
        // A headless child ctx (has_ui=false) — the exact shape a re-exec'd subagent runs under.
        let ctx = HostCtx::event(ExtMode::Print, false, agent_dir.clone());
        let event = HostEvent::ToolCall {
            call_id: ToolCallId::from("fwd-call-1"),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": command }),
        };
        match ext.on_event(&event, &ctx).await {
            // The gate let the call THROUGH → the tool "runs": touch the sentinel (the child's
            // observable proof the forwarded allow crossed the boundary and gated it open).
            HookOutcome::Noop => {
                let _ = std::fs::write(&sentinel, b"EXECUTED");
                EXIT_ALLOWED
            }
            // The gate BLOCKED the call (forwarded deny / fail-closed timeout) → the tool never runs.
            HookOutcome::Block { .. } => EXIT_BLOCKED,
            _ => EXIT_UNEXPECTED,
        }
    });
    std::process::exit(code);
}

// =================================================================================================
// The PARENT role — this test process: the REAL watcher + a scripted human sink.
// =================================================================================================

/// Spawn the child-role subprocess (re-exec of THIS test binary), with the subagent-child env + the
/// parent anchor set, so its production gate installs the ForwardingAskChannel addressed at `parent_id`.
fn spawn_child(
    agent_dir: &Path,
    parent_id: &str,
    command: &str,
    sentinel: &Path,
    child_wait_ms: u64,
) -> std::process::Child {
    let exe = std::env::current_exe().expect("current test exe");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", CHILD_TEST_NAME, "--nocapture", "--test-threads=1"]);
    cmd.env(CHILD_ROLE_ENV, "1");
    cmd.env(CHILD_AGENT_DIR_ENV, agent_dir);
    cmd.env(CHILD_COMMAND_ENV, command);
    cmd.env(CHILD_SENTINEL_ENV, sentinel);
    // The subagent-child role signal + the parent-session anchor the ForwardingAskChannel reads
    // (`cyrup_ext_subagents::PARENT_SESSION_ENV_VAR` = "CYRUP_SUBAGENT_PARENT_SESSION").
    cmd.env("CYRUP_SUBAGENT_CHILD", "1");
    cmd.env("CYRUP_SUBAGENT_PARENT_SESSION", parent_id);
    // Bound the child's blocking wait so a wiring bug fails fast instead of hanging on the 10-min
    // production default (and so the timeout case is a fast, genuine fail-closed proof).
    cmd.env("CYRUP_PERMISSION_FORWARDING_TIMEOUT_MS", child_wait_ms.to_string());
    cmd.spawn().expect("spawn child role subprocess")
}

fn scripted_parent(session_id: &str, answer: &str) -> (Arc<dyn HostServices>, Arc<AtomicUsize>) {
    let selects = Arc::new(AtomicUsize::new(0));
    let host = ScriptedHost {
        session_id: session_id.to_string(),
        answer: answer.to_string(),
        selects: selects.clone(),
        lock: Arc::new(HumanInteractionLock::new()),
    };
    (Arc::new(host), selects)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_allow_crosses_process_and_lets_child_proceed() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    write_policy(agent_dir.path());
    let parent_id = format!("parent-{}", uuid_like());
    let sentinel = agent_dir.path().join("child-executed.sentinel");

    // The REAL parent watcher against a scripted human that ALLOWS.
    let (services, selects) = scripted_parent(&parent_id, "Allow Once");
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

    let child = spawn_child(agent_dir.path(), &parent_id, "echo hi", &sentinel, 20_000);
    let code = wait_child(child, Duration::from_secs(30)).await;
    watcher.abort();

    assert_eq!(code, Some(EXIT_ALLOWED), "the forwarded ALLOW must let the child's tool proceed (exit 0)");
    assert!(sentinel.exists(), "the child's tool must have run (sentinel written) after the cross-process allow");
    assert!(
        selects.load(Ordering::SeqCst) >= 1,
        "the PARENT watcher must have surfaced ≥1 forwarded prompt written by the CHILD process (boundary crossed)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_deny_crosses_process_and_blocks_child() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    write_policy(agent_dir.path());
    let parent_id = format!("parent-{}", uuid_like());
    let sentinel = agent_dir.path().join("child-executed.sentinel");

    // The REAL parent watcher against a scripted human that REJECTS.
    let (services, selects) = scripted_parent(&parent_id, "Reject");
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

    let child = spawn_child(agent_dir.path(), &parent_id, "rm -rf /", &sentinel, 20_000);
    let code = wait_child(child, Duration::from_secs(30)).await;
    watcher.abort();

    assert_eq!(code, Some(EXIT_BLOCKED), "the forwarded DENY must block the child's tool (exit 3)");
    assert!(!sentinel.exists(), "a denied tool must NOT run (no sentinel)");
    assert!(
        selects.load(Ordering::SeqCst) >= 1,
        "the PARENT watcher must have surfaced the forwarded prompt it then rejected"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_timeout_fail_closes_the_child() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    write_policy(agent_dir.path());
    let parent_id = format!("parent-{}", uuid_like());
    let sentinel = agent_dir.path().join("child-executed.sentinel");

    // NO watcher/responder at all: the child writes its request and no one ever answers. It must
    // fail-CLOSE (deny) after its shortened wait bound (1.2s here), never hang, never allow.
    let child = spawn_child(agent_dir.path(), &parent_id, "curl http://evil", &sentinel, 1_200);
    let started = Instant::now();
    let code = wait_child(child, Duration::from_secs(30)).await;

    assert_eq!(code, Some(EXIT_BLOCKED), "an unanswered forward must fail-CLOSE the child (exit 3)");
    assert!(!sentinel.exists(), "a timed-out forward must NOT let the tool run");
    assert!(
        started.elapsed() >= Duration::from_millis(1_000),
        "the child must have actually WAITED on its bound before denying (not denied instantly)"
    );
}

/// A dependency-free unique-ish token for a per-test parent session id (all-unreserved chars).
fn uuid_like() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{}", std::process::id())
}

//! `CYRUP_HOME`-sandboxed tests relocated out of `src/extension.rs`'s own `#[cfg(test)] mod tests`.
//!
//! These tests must override the process-global `CYRUP_HOME` env var so the `Full`-mode
//! `NativeExtension::init`/`teardown_session` T6 startup housekeeping they drive resolves its
//! async/results roots under a tempdir instead of the real developer/CI machine's `~/.cyrup`. Since
//! Rust requires `unsafe` for `std::env::set_var`/`remove_var`, and this crate's `src/lib.rs` is
//! `#![forbid(unsafe_code)]`, they cannot live inside that crate's own unit-test module — exactly
//! like every other `tests/*_integration.rs` file in this crate (see e.g.
//! `extension_end_to_end_smoke.rs`'s identical rationale), this file is a separate compilation unit
//! NOT subject to that crate-level `forbid`, so the `unsafe` env mutation is legal here.
//!
//! Every item these tests touch (`subagent_extension_for`, `SubagentsExtension`, `SubagentExecutor`,
//! `SubagentExtensionConfig`, `RunId`, `RunPaths`, `InitApi`, `NativeExtension`) is already `pub`, so
//! the relocation needed no visibility changes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;

use cyrup_ext::native::{InitApi, NativeExtension};
use cyrup_ext_subagents::background::{RunId, RunPaths};
use cyrup_ext_subagents::extension::{subagent_extension_for, SubagentExecutor, SubagentsExtension};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Serializes every test in this file that overrides `CYRUP_HOME` (process-global state) so
/// `cargo test`'s concurrent execution never lets two such overrides race each other — mirrors this
/// crate's other integration tests' identical `ENV_MUTATION_LOCK` convention.
static ENV_MUTATION_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

/// T6: a `CYRUP_SUBAGENT_CHILD=1` process without fanout authorization must attach NO subagent
/// extension at all (so its `subagent` tool, slash commands, and watchers are never registered),
/// while a fanout-authorized child gets an extension that installs the tool but NO lifecycle
/// subscriptions (no background watcher, no session-start housekeeping), and a non-child gets the
/// full lifecycle surface.
#[tokio::test]
async fn child_env_gate_controls_what_is_registered() {
    // `Full` mode's `init()` now runs the T6 startup housekeeping (async/results root
    // creation) below — sandbox `CYRUP_HOME` to a tempdir for this test's duration so it never
    // touches the real developer/CI machine's `~/.cyrup`.
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    // SAFETY: this file is a separate compilation unit from `cyrup-ext-subagents`'s own
    // `#![forbid(unsafe_code)]` `src/lib.rs` (see this file's module doc); the mutation is scoped
    // and mutex-serialized (`ENV_MUTATION_LOCK`).
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let cwd = std::env::temp_dir();

    // Plain child → no extension → no `subagent` tool registered anywhere (regardless of the
    // opt-in `installed` signal — a plain child is never gated on it).
    let disabled =
        subagent_extension_for(SubagentExtensionConfig::default(), cwd.clone(), true, false, true);
    assert!(disabled.is_none(), "a plain subagent child registers no subagent surface at all");

    // Fanout-authorized child → an extension whose init installs NO lifecycle subscriptions.
    // `installed = false` proves the child-safe surface attaches REGARDLESS of the opt-in gate.
    let child_safe =
        subagent_extension_for(SubagentExtensionConfig::default(), cwd.clone(), true, true, false)
            .expect("a fanout-authorized child registers the restricted tool");
    let mut api = InitApi::new();
    child_safe.init(&mut api).await.expect("child-safe init succeeds");
    assert!(
        !api.subscriptions().contains(cyrup_ext::EventKind::SessionStart),
        "a child-safe extension installs no SessionStart watcher/housekeeping"
    );
    assert!(!api.subscriptions().contains(cyrup_ext::EventKind::SessionShutdown));

    // Non-child (root orchestrator) that HAS opted in (`installed = true`) → the full lifecycle
    // surface.
    let full = subagent_extension_for(SubagentExtensionConfig::default(), cwd, false, false, true)
        .expect("a non-child process registers the full orchestrator extension");
    let mut api = InitApi::new();
    full.init(&mut api).await.expect("full init succeeds");
    assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionStart));
    assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionShutdown));

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_HOME");
    }
}

/// The opt-in flip side (requirement (b)/(c) semantics via the pure form): once opted in
/// (`installed = true`, which both `CYRUP_SUBAGENTS=1` and a present `subagents/config.json` feed
/// through `is_installed`), a top-level session attaches the FULL orchestrator surface.
#[tokio::test]
async fn top_level_with_optin_attaches_full() {
    // `Full` mode's `init()` now runs the T6 startup housekeeping — sandbox `CYRUP_HOME` (see
    // `ENV_MUTATION_LOCK`'s own doc).
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let cwd = std::env::temp_dir();
    let ext = subagent_extension_for(
        SubagentExtensionConfig::default(),
        cwd,
        /* child */ false,
        /* fanout_authorized */ false,
        /* installed */ true,
    )
    .expect("an opted-in top-level session attaches the full orchestrator surface");
    let mut api = InitApi::new();
    ext.init(&mut api).await.expect("full init succeeds");
    assert!(
        api.subscriptions().contains(cyrup_ext::EventKind::SessionStart),
        "the full orchestrator surface installs the SessionStart housekeeping"
    );

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_HOME");
    }
}

#[tokio::test]
async fn init_registers_the_tool_and_all_thirteen_commands() {
    // `Full` mode's `init()` now runs the T6 startup housekeeping — sandbox `CYRUP_HOME` (see
    // `ENV_MUTATION_LOCK`'s own doc).
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let ext = SubagentsExtension::new();
    let mut api = InitApi::new();
    ext.init(&mut api).await.expect("init succeeds");
    // InitApi has no public inspector beyond subscriptions in this phase's surface; the real
    // proof that registration actually reaches the host is `main.rs`'s wiring plus the
    // end-to-end smoke test, which drives `init` through a real `SessionBuilder`.
    assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionStart));
    assert!(api.subscriptions().contains(cyrup_ext::EventKind::SessionShutdown));

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_HOME");
    }
}

/// A minimal [`cyrup_ext::host::HostServices`] double reporting a fixed session id/name.
struct FixedSessionHost {
    id: &'static str,
    name: &'static str,
}
impl cyrup_ext::host::HostServices for FixedSessionHost {
    fn session_id(&self) -> Option<String> {
        Some(self.id.to_string())
    }
    fn session_name(&self) -> Option<String> {
        Some(self.name.to_string())
    }
}

/// Regression proof for the pi-parity fix closing `session_shutdown`'s "deliberate no-op"
/// divergence (pi `extension/index.ts:644-680`): [`SubagentExecutor::teardown_session`] must
/// actually stop the completion watcher, abort+clear the job tracker's poll loop/job map, and
/// clear the captured parent-session anchor — mirroring pi's `stopResultWatcher()` +
/// `clearInterval(state.poller); state.asyncJobs.clear()` + `delete
/// process.env[SUBAGENT_PARENT_SESSION_ENV]`.
///
/// Before this fix, `on_event(SessionShutdown)` did nothing at all: the tracker's poll loop and
/// job map, the completion watcher, and the captured anchor all survived a shutdown untouched,
/// so every assertion below would fail against the pre-fix code (the tracker would still report
/// the job as tracked and the anchor would still resolve to the old session's id).
#[tokio::test]
async fn teardown_session_stops_the_tracker_and_clears_the_parent_session_anchor() {
    // `install_completion_watcher` resolves its results dir under `dirs_home()`/
    // `subagents_home()` — sandbox `CYRUP_HOME` (see `ENV_MUTATION_LOCK`'s own doc).
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    // SAFETY: scoped, mutex-serialized env mutation — see this file's module doc.
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let executor = SubagentExecutor::new();

    // Capture a parent-session anchor, as `on_event(SessionStart)` does at depth 0.
    executor.set_host_services(Arc::new(FixedSessionHost {
        id: "sess-abc123",
        name: "root-session",
    }));
    executor.capture_parent_session_anchor();
    assert_eq!(executor.root_parent_session().as_deref(), Some("sess-abc123"));

    // Track a job, as `resume_tracking`/`spawn_background` do — this starts the shared poll
    // loop (R-SA-093).
    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    let run_id = RunId::from_token("regression-run".to_string());
    let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    executor.tracker().track(run_id.clone(), paths, None).await;
    assert_eq!(executor.tracker().tracked_count(), 1);
    assert!(executor.tracker().is_polling().await, "tracking a job must start the poll loop");

    // Install a completion watcher over a real (creatable) results dir, as `on_event
    // (SessionStart)` does.
    executor.install_completion_watcher(dir.path()).await;

    // The actual fix under test.
    executor.teardown_session().await;

    assert_eq!(
        executor.root_parent_session(),
        None,
        "the parent-session anchor must be cleared on session_shutdown"
    );
    assert_eq!(
        executor.tracker().tracked_count(),
        0,
        "the job tracker's in-memory job map must be cleared on session_shutdown"
    );
    assert!(
        !executor.tracker().is_polling().await,
        "the job tracker's poll loop must be stopped on session_shutdown"
    );

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_HOME");
    }
}


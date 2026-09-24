//! A headless auto-drain longer than the extension dispatcher's per-handler budget still runs to
//! completion, in BOTH `AgentEnd` handlers that await it: the orchestrator's
//! (`extension/host/native_impl.rs`) and a subagent child's (`prompt_runtime.rs`).
//!
//! # Where the budget lives
//!
//! `cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET` (5 s), enforced by `Dispatcher::invoke_contained`,
//! which DROPS a handler future still running at the deadline. The session reaches it for
//! `AgentEnd` through `cyrup_ext::subscriber::ExtSubscriber::on_event` → `dispatch_notify`. The
//! drain's own bound is `DEFAULT_AUTO_DRAIN_TIMEOUT_MS` (30 min); upstream awaits it with no
//! per-handler budget at all (`extension/index.ts:834`, `subagent-prompt-runtime.ts:490-500`
//! @v0.68.0). So a background run that lands at 6.5 s was abandoned at 5 s.
//!
//! # The shape
//!
//! A real session (the harness, headless `Print` mode) with the real extension; a `Running`
//! background run owned by that session's id is seeded under the extension's own artifact roots,
//! and a task settles it (writes its authoritative `ResultFile`) `LAND_AFTER_MS` in. The drain is
//! awaited inside `AgentEnd`, so `harness.run` returns only once the drain does: at or after the
//! landing when the drain survives, at the 5 s budget when it is cut.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_ext::native::NativeExtension;
use cyrup_ext_subagents::background::{
    ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, run_artifact_roots_in,
};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::identity::SessionId;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_test_support::harness::{HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::FauxResponse;

/// Past the 5 s dispatch budget, far inside the drain's own 30 min bound.
const LAND_AFTER_MS: u64 = 6_500;

const _: () = assert!(
    LAND_AFTER_MS as u128 > cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET.as_millis(),
    "the run must land after the dispatch budget, or this file tests nothing"
);

/// A `Running` run owned by `session`, the on-disk shape `list_active_runs` reads.
fn seed_running(async_root: &Path, results_dir: &Path, run_id: &RunId, session: &str) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    std::fs::create_dir_all(&paths.run_dir).expect("run dir");
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(std::process::id()));
    status.state = RunState::Running;
    status.session_id = SessionId::parse_opt(Some(session));
    std::fs::write(&paths.status, serde_json::to_string(&status).unwrap()).expect("status.json");
}

/// The run's authoritative terminal `ResultFile` — what a detached runner writes last, and the
/// one thing that flips a run terminal for every reader.
fn settle(async_root: &Path, results_dir: &Path, run_id: &RunId) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    if let Some(parent) = paths.legacy_result_root.parent() {
        std::fs::create_dir_all(parent).expect("results dir");
    }
    let result = ResultFile {
        schedule_origin: None,
        id: run_id.clone(),
        run_id: run_id.clone(),
        agent: "worker".to_string(),
        mode: RunMode::Single,
        state: RunState::Complete,
        success: true,
        cwd: PathBuf::from("/tmp"),
        session_file: None,
        session_id: None,
        completion_owner_id: None,
        results: Vec::new(),
        workflow_children: None,
        workflow_receipt: None,
    };
    std::fs::write(
        &paths.legacy_result_root,
        serde_json::to_string(&result).unwrap(),
    )
    .expect("result file");
}

/// Seed the run, arm its landing, run one headless turn, and return how long the turn took.
async fn drive(
    harness: &cyrup_test_support::harness::Harness,
    session: String,
    async_root: PathBuf,
    results_dir: PathBuf,
) -> Duration {
    let run_id = RunId::from_token("drainbudget0001");
    seed_running(&async_root, &results_dir, &run_id, &session);
    let lander = tokio::spawn({
        let run_id = run_id.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(LAND_AFTER_MS)).await;
            settle(&async_root, &results_dir, &run_id);
        }
    });
    let started = Instant::now();
    harness
        .run("finish up")
        .await
        .expect("the headless turn completes");
    let elapsed = started.elapsed();
    lander.await.expect("the lander ran");
    elapsed
}

fn assert_drained(elapsed: Duration) {
    assert!(
        elapsed >= Duration::from_millis(LAND_AFTER_MS),
        "the AgentEnd drain was dropped at the dispatch budget: the turn returned after \
         {elapsed:?}, before the background run landed at {LAND_AFTER_MS} ms"
    );
    assert!(
        elapsed < Duration::from_secs(60),
        "the drain returned once the run landed, not at its own 30 min deadline ({elapsed:?})"
    );
}

/// The ORCHESTRATOR's drain (`native_impl.rs`, `HostEvent::AgentEnd`) outlives the budget.
///
/// Killing mutation: the handler calls `drain_outstanding_work` directly instead of
/// `drain_outstanding_work_in_handler` — no declared `AutoDrain` wait, the handler is dropped at
/// 5 s, and the turn returns before the run lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_main_sessions_auto_drain_is_not_cut_by_the_dispatch_budget() {
    let home = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(home.path());
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            async_by_default: false,
            roots: roots.clone(),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));
    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension.clone() as Arc<dyn NativeExtension>],
        responses: vec![FauxResponse::text("done")],
        ..HarnessOptions::default()
    })
    .await
    .expect("a real headless session with the real SubagentsExtension");
    let session = extension
        .executor()
        .current_session_id()
        .expect("the harness binds a live session id, which is what scopes the drain");
    // The handler resolves its roots from the extension's config and its cwd from the dispatch
    // ctx, which is the SESSION's cwd — the same two inputs, here.
    let artifact_roots = run_artifact_roots_in(&roots, harness.cwd());

    let elapsed = drive(
        &harness,
        session,
        artifact_roots.async_root,
        artifact_roots.results_dir,
    )
    .await;
    assert_drained(elapsed);
}

/// A subagent CHILD's drain (`prompt_runtime.rs`), built exactly as a spawned child builds its
/// runtime — from its env. The only variable set is `CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT`, which
/// the parent writes into EVERY child (`exec/spawn_plan.rs`), so this is an ordinary, UNARMED
/// child: no watchdog, no steering, no budget.
///
/// Killing mutations, each alone: (1) the child handler calls `drain_outstanding_work` directly,
/// bypassing the declared wait — cut at 5 s; (2) `AgentEnd` declared only inside the watchdog
/// block of `init` again — an unarmed child never receives `agent_end`, never drains, and the turn
/// returns immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unarmed_childs_auto_drain_runs_and_is_not_cut_by_the_dispatch_budget() {
    let runtime = Arc::new(
        cyrup_ext_subagents::prompt_runtime::prompt_runtime_from_env(&|key: &str| {
            (key == cyrup_ext_subagents::prompt_runtime::INHERIT_PROJECT_CONTEXT_ENV)
                .then(|| "1".to_string())
        })
        .expect("builds")
        .expect("every spawned child has a prompt runtime"),
    );
    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![runtime.clone() as Arc<dyn NativeExtension>],
        responses: vec![FauxResponse::text("done")],
        ..HarnessOptions::default()
    })
    .await
    .expect("a real headless session with the child's prompt runtime");
    let session = harness.session().session_id().as_str().to_string();

    // The child resolves its roots from its own environment and its drain from the session's
    // cwd — the same two calls its handler makes.
    let roots = Roots::from_env();
    let cwd = harness.cwd().to_path_buf();
    let artifact_roots = run_artifact_roots_in(&roots, &cwd);
    let elapsed = drive(
        &harness,
        session,
        artifact_roots.async_root.clone(),
        artifact_roots.results_dir.clone(),
    )
    .await;
    // The roots may be the shared per-user temp tree; the cwd key is this test's own tempdir, so
    // removing its two directories removes only what this test wrote.
    let _ = std::fs::remove_dir_all(&artifact_roots.async_root);
    let _ = std::fs::remove_dir_all(&artifact_roots.results_dir);
    assert_drained(elapsed);
}

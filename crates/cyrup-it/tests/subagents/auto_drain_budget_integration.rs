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

/// SUBA-187 Verify — pi `3bb9b203` / #2666 (`extension/index.ts:728` @ad11b7ab): after the
/// headless `agent_end` drain, `resultWatcher.deliverPendingResults` hands every finished result
/// to the session at once, so the queued completion turn runs before the headless host returns.
///
/// Driven through print mode itself — `cyrup_modes::run_print` over an `AgentSessionRuntime`
/// carrying the real extension, which is what `cyrup -p` runs. (The test-support `Harness` cannot
/// show this: it holds its session by value, and only a shared session gets the injection pump a
/// completion is delivered through, `AgentSession::into_shared`.) A `Running` background run owned
/// by the session lands its terminal, INDEXED result (stage → index → promote, as `finish_run`
/// writes it) while the turn's `agent_end` drain is waiting on it. When `run_print` returns, the
/// session must already hold the `subagent-notify` message and have run the completion turn.
///
/// RED at HEAD: the drain returned as soon as the run was terminal, the completion still had the
/// results watcher's 500 ms poll and the batcher's 650 ms debounce ahead of it, and `run_print`
/// returned without it — the result stayed on disk and the provider was called once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_that_lands_during_the_headless_drain_is_delivered_before_print_mode_returns() {
    use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let roots = Roots::sandboxed(&tmp.path().join("home"));
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            async_by_default: false,
            roots: roots.clone(),
            ..SubagentExtensionConfig::default()
        },
        cwd.clone(),
    ));
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("done")], cyrup_sdk::core::StopReason::Stop),
        faux_assistant_message(
            vec![faux_text("noted the result")],
            cyrup_sdk::core::StopReason::Stop,
        ),
    ]);
    let mut config = cyrup_sdk::SessionConfig::new(cwd.clone(), agent_dir);
    config.trust_override = Some(true);
    let target = config.target.clone();
    let provider: Arc<dyn cyrup_provider::Provider> = faux.clone();
    let factory = Arc::new(
        cyrup_sdk::SessionFactory::new(provider, config)
            .with_native_extension(extension.clone() as Arc<dyn NativeExtension>),
    );
    let runtime = cyrup_sdk::AgentSessionRuntime::create(factory, target)
        .await
        .expect("a real print-mode runtime with the real SubagentsExtension");
    let session = extension
        .executor()
        .current_session_id()
        .expect("the runtime binds a live session id");
    let roots_here = run_artifact_roots_in(&roots, &cwd);
    let (async_root, results_dir) = (roots_here.async_root, roots_here.results_dir);
    std::fs::create_dir_all(&results_dir).expect("results dir");

    let run_id = RunId::from_token("drainlands000001");
    seed_running(&async_root, &results_dir, &run_id, &session);
    let lander = tokio::spawn({
        let (async_root, results_dir, run_id, session) = (
            async_root.clone(),
            results_dir.clone(),
            run_id.clone(),
            session.clone(),
        );
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            land_indexed(&async_root, &results_dir, &run_id, &session).await;
        }
    });

    let (mut out, mut err) = (Vec::new(), Vec::new());
    cyrup_sdk::run_print(
        &runtime,
        std::iter::once(cyrup_sdk::UserInput::text(
            "finish up",
            cyrup_sdk::InputSource::Cli,
        )),
        &mut out,
        &mut err,
        cyrup_sdk::PrintOptions::default(),
    )
    .await
    .expect("print mode completes");
    lander.await.expect("the lander ran");

    // The session's model context: the `subagent-notify` custom message reaches it as the
    // completion notice's text (`format_completion_message`).
    let notified = runtime
        .session()
        .await
        .messages()
        .await
        .iter()
        .filter_map(|message| serde_json::to_string(message).ok())
        .any(|line| line.contains("Background task completed: **worker**"));
    assert!(
        notified,
        "the completion that landed during the drain must reach the session before print mode \
         returns"
    );
    assert_eq!(
        faux.call_count(),
        2,
        "and the completion turn it triggers has run before print mode returns"
    );
}

/// What a detached runner does last for a successful run: flip `status.json` terminal and write
/// the session-owned result through the index.
async fn land_indexed(async_root: &Path, results_dir: &Path, run_id: &RunId, session: &str) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(std::process::id()));
    status.state = RunState::Complete;
    status.session_id = SessionId::parse_opt(Some(session));
    std::fs::write(&paths.status, serde_json::to_string(&status).unwrap()).expect("status.json");
    let session_id = SessionId::parse(session).expect("session id");
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
        session_id: Some(session_id.clone()),
        // A run this process launched records this process's owner id; only that makes the
        // completion this instance's to deliver (`Attribution::Ours`).
        completion_owner_id: Some(cyrup_ext_subagents::identity::current_completion_owner_id()),
        results: Vec::new(),
        workflow_children: None,
        workflow_receipt: None,
    };
    let written_at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    cyrup_ext_subagents::background::result_index::write_async_result_file(
        &cyrup_ext_subagents::background::result_index::ResultWrite {
            results_dir,
            session_id: &session_id,
            run_id,
            written_at,
            async_dir: None,
            tool_call_id: None,
        },
        &result,
    )
    .await
    .expect("indexed result");
}

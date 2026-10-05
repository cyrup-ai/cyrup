//! SUBA-149, the residual half — a caller who asks for isolation on the DEFAULT path must get it.
//!
//! # The shape that was still broken
//!
//! `subagents.asyncByDefault` is `true` (`registration/mod.rs`, whose own doc says *"absent and
//! `true` both mean background"*), so a bare `subagent({agent, task, worktree: true})` is a
//! BACKGROUND launch. Upstream puts `worktree: true` on the one step `executeAsyncSingle` spawns
//! (`runs/background/async-execution.ts:2163` @v0.75.0) and lets its runner allocate from that
//! step (`runs/background/subagent-runner.ts:4440-4443`); a `worktree: true` SEQUENTIAL chain step
//! reaches the identical branch (`async-execution.ts:1336-1340` resolves its expected agent cwd
//! from `sequential.worktree`).
//!
//! cyrup's `SingleStepSpec` had no `worktree` field at all, and the flag was consumed on
//! `ParallelGroupSpec` alone. So the async single run and every sequential chain step dropped the
//! request silently — no isolation, no diagnostic. The foreground single run was fixed first
//! (commit `b9e35f5d`), and foreground is the MINORITY shape for a bare call.
//!
//! # What each test here pins
//!
//! * the headline — a bare call with NO `async: false` reaches hop 2 carrying the request, and the
//!   reply proves the async path was the one taken;
//! * the walker actually runs the child inside the allocated worktree, and a child that asked for
//!   nothing still runs in the shared cwd;
//! * a sequential chain step is isolated, independently of its neighbours;
//! * settle: preserve-then-publish-then-remove, upstream's own order, and a DETACHED child's tree
//!   is left alone because that child is still writing into it;
//! * Fix 2(b): the async-single admission probe, with upstream's own refusal sentences.
//!
//! Every git repository and every worktree base directory here is a `TempDir` owned by the test.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::Mutex;

use cyrup_core::CancelToken;

use crate::error::SubagentError;
use crate::spawn::chain_graph::{
    ChainRunContext, HandoffBinding, OutputRegistry, RunnerStep, SingleStepExecutor,
    SingleStepSpec, StepResult, StepSlot, walk_chain,
};
use crate::spawn::parallel::GlobalConcurrencyLimit;
use crate::spawn::worktree::WorktreeRequest;

// -------------------------------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------------------------------

fn git_ok(cwd: &Path, args: &[&str]) {
    let out = StdCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A real, CLEAN git repository — the only source a managed allocation accepts.
fn clean_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git_ok(dir.path(), &["init", "-q"]);
    git_ok(dir.path(), &["config", "user.email", "t@example.com"]);
    git_ok(dir.path(), &["config", "user.name", "SUBA-149 Tests"]);
    std::fs::write(dir.path().join("tracked.txt"), "initial\n").expect("tracked");
    git_ok(dir.path(), &["add", "-A"]);
    git_ok(dir.path(), &["commit", "-q", "-m", "initial"]);
    dir
}

fn step(agent: &str, task: &str, worktree: WorktreeRequest) -> SingleStepSpec {
    SingleStepSpec {
        worktree,
        machine: None,
        agent: agent.to_string(),
        task: task.to_string(),
        skills: None,
        session_dir: None,
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        fast: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
        label: None,
        session_name: None,
    }
}

/// What ONE dispatch observed, recorded by [`Observer`]: the cwd the walker handed the step, and
/// whether that directory existed when the child "ran".
#[derive(Clone, Debug)]
struct Observed {
    agent: String,
    cwd: PathBuf,
}

/// A [`SingleStepExecutor`] that records the EFFECTIVE cwd of every dispatch — `step.cwd` when the
/// walker set one, else the context's own, which is exactly how the production adapter resolves it
/// (`background::runner_main::executor`'s `step.cwd.clone().unwrap_or_else(|| ctx.cwd.clone())`).
/// Optionally writes a file into that directory, so a settle test has real work to harvest, and
/// optionally reports the dispatch as DETACHED.
struct Observer {
    seen: Mutex<Vec<Observed>>,
    write_file: Option<&'static str>,
    detached: bool,
}

impl Observer {
    fn new() -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            write_file: None,
            detached: false,
        }
    }

    fn writing(name: &'static str) -> Self {
        Self {
            write_file: Some(name),
            ..Self::new()
        }
    }

    fn detaching(name: &'static str) -> Self {
        Self {
            write_file: Some(name),
            detached: true,
            ..Self::new()
        }
    }

    fn observed(&self) -> Vec<Observed> {
        self.seen.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl SingleStepExecutor for Observer {
    async fn run_single(
        &self,
        spec: &SingleStepSpec,
        _resolved_task: &str,
        ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError> {
        let cwd = spec.cwd.clone().unwrap_or_else(|| ctx.cwd.clone());
        if let Some(name) = self.write_file {
            std::fs::write(cwd.join(name), "the child's work\n").expect("the child can write");
        }
        self.seen.lock().expect("not poisoned").push(Observed {
            agent: spec.agent.clone(),
            cwd,
        });
        let mut result = StepResult::success(Some("done".to_string()), None);
        result.exit_code = Some(0);
        result.detached = self.detached;
        Ok(result)
    }
}

/// A context rooted at `repo`, allocating into `base`, publishing (or not) into `manifest`.
fn ctx_in(repo: &Path, base: &Path, manifest: Option<PathBuf>) -> ChainRunContext {
    ChainRunContext {
        cwd: repo.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        cancel: CancelToken::new(),
        global_limit: GlobalConcurrencyLimit::default_limit(),
        worktree_base_dir: Some(base.to_path_buf()),
        original_task: "do everything".to_string(),
        chain_dir: None,
        dynamic_fanout_max_items: None,
        step_slot: StepSlot::Exclusive(0),
        handoff: manifest.map(|manifest_path| HandoffBinding {
            manifest_path,
            run_id: crate::handoff::LaneId::parse("suba149run").expect("a legal lane id"),
            // The ASYNC runner's own shape for a single-step run — pi `mode: "single"`,
            // `source: "async"` (`subagent-runner.ts:4741-4744`).
            mode: crate::handoff::HandoffMode::Single,
            source: crate::handoff::HandoffSource::Async,
        }),
    }
}

/// `git -C <dir> rev-parse --show-toplevel`, i.e. "which work tree is this directory in".
fn toplevel_of(dir: &Path) -> PathBuf {
    let out = StdCommand::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("git runs");
    assert!(out.status.success(), "{dir:?} must be inside a work tree");
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// -------------------------------------------------------------------------------------------
// The walker: allocation, the control, and the sequential chain step
// -------------------------------------------------------------------------------------------

/// **The walker half of the headline.** A `SingleStep` carrying
/// [`WorktreeRequest::Isolated`] runs in its OWN managed git worktree, and the control — a step
/// that asked for nothing — runs in the shared cwd of the same walk.
///
/// pi `singleCwd = singleWorktreeSetup?.worktrees[0]?.agentCwd ?? cwd`
/// (`subagent-runner.ts:4511`) then `bindWorktreeCwd({ ...seqStep, cwd: singleCwd }, singleCwd)`
/// (`:4536-4537`).
///
/// **Gutting mutation this fails on:** removing the `ManagedLaunch::resolve` hand-off in
/// `chain_graph::run_single_step` (dispatching `spec` rather than the allocation's `child_cwd()`)
/// — the isolated step's cwd then equals the repository root, which is the whole defect.
#[tokio::test]
async fn an_isolated_single_step_runs_in_a_worktree_and_a_shared_one_does_not() {
    let repo = clean_repo();
    let base = tempfile::tempdir().expect("base");
    let graph = vec![
        RunnerStep::SingleStep(step("isolated", "work alone", WorktreeRequest::Isolated)),
        RunnerStep::SingleStep(step("shared", "work in place", WorktreeRequest::Shared)),
    ];
    let observer = Arc::new(Observer::new());
    let executor: Arc<dyn SingleStepExecutor> = observer.clone();
    let mut registry = OutputRegistry::new();

    let (results, _groups) = walk_chain(
        &graph,
        &mut registry,
        &executor,
        // No manifest: this test is about WHERE the child ran, and nothing else.
        &ctx_in(repo.path(), base.path(), None),
    )
    .await
    .expect("the walk succeeds");
    assert!(results.iter().all(|r| r.success), "{results:?}");

    let seen = observer.observed();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let isolated = &seen[0];
    let shared = &seen[1];
    assert_eq!(isolated.agent, "isolated");
    assert_eq!(shared.agent, "shared");

    assert_ne!(
        isolated.cwd,
        repo.path(),
        "the isolation request must not resolve to the shared cwd — that IS SUBA-149"
    );
    assert!(
        isolated.cwd.starts_with(base.path()),
        "the allocation must land under the configured worktree base dir: {isolated:?}"
    );
    assert_eq!(
        toplevel_of(&isolated.cwd),
        isolated.cwd.canonicalize().expect("the worktree exists"),
        "the child's cwd must be its OWN work tree's root, not the source repository's"
    );
    assert_eq!(
        shared.cwd.canonicalize().expect("the repo exists"),
        repo.path().canonicalize().expect("the repo exists"),
        "a child that did not ask for isolation must still run in the shared cwd"
    );
}

/// A SEQUENTIAL chain step with `worktree: true` is isolated, and its neighbours are not — pi
/// `sequential.worktree` (`shared/settings.ts:50`, read at `async-execution.ts:1336` and
/// allocated at `subagent-runner.ts:4441`).
///
/// Two isolated steps in ONE walk also prove the allocation ids are distinct: upstream names them
/// `${id}-s${stepIndex}`, and a shared id would make the second `git worktree add` fail on an
/// existing path.
///
/// **Gutting mutation this fails on:** the same removal as the test above; additionally, making
/// `chain_graph::worktree_allocation_id` ignore the step position fails the second allocation
/// outright.
#[tokio::test]
async fn a_sequential_chain_step_with_worktree_true_is_isolated_independently() {
    let repo = clean_repo();
    let base = tempfile::tempdir().expect("base");
    let graph = vec![
        RunnerStep::SingleStep(step("first", "plain step", WorktreeRequest::Shared)),
        RunnerStep::SingleStep(step("second", "isolated step", WorktreeRequest::Isolated)),
        RunnerStep::SingleStep(step("third", "another isolated", WorktreeRequest::Isolated)),
    ];
    let observer = Arc::new(Observer::new());
    let executor: Arc<dyn SingleStepExecutor> = observer.clone();
    let mut registry = OutputRegistry::new();

    let (results, _groups) = walk_chain(
        &graph,
        &mut registry,
        &executor,
        &ctx_in(repo.path(), base.path(), None),
    )
    .await
    .expect("the walk succeeds");
    assert!(results.iter().all(|r| r.success), "{results:?}");

    let seen = observer.observed();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(
        seen[0].cwd.canonicalize().unwrap(),
        repo.path().canonicalize().unwrap(),
        "the un-isolated step stays in the shared cwd"
    );
    assert!(seen[1].cwd.starts_with(base.path()), "{seen:?}");
    assert!(seen[2].cwd.starts_with(base.path()), "{seen:?}");
    assert_ne!(
        seen[1].cwd, seen[2].cwd,
        "two isolated steps of one chain must not share a worktree"
    );
}

/// The AUTHORED chain surface: a `.chain.json` sequential step declaring `worktree: true` must
/// lower onto the spec rather than being read for the parallel shape only.
///
/// `ChainStepConfig::worktree` already existed and was consumed at the `parallel` branch alone, so
/// the key was accepted, validated and then dropped for the sequential shape.
///
/// **Gutting mutation this fails on:** reverting
/// `discovery::chains::chain_step_to_single_step_spec`'s `worktree` line to `Shared`.
#[test]
fn an_authored_sequential_chain_step_lowers_its_worktree_flag() {
    let config: crate::discovery::types::ChainStepConfig =
        serde_json::from_value(serde_json::json!({
            "agent": "builder",
            "task": "build it",
            "worktree": true
        }))
        .expect("a sequential step parses");
    match crate::discovery::chains::chain_step_to_runner_step(&config, 4) {
        RunnerStep::SingleStep(spec) => assert_eq!(
            spec.worktree,
            WorktreeRequest::Isolated,
            "an authored `worktree: true` sequential step must reach the walker as a request"
        ),
        other => panic!("a sequential step lowers to a SingleStep, got {other:?}"),
    }
}

// -------------------------------------------------------------------------------------------
// Settle: preserve, publish, remove — and the detached exemption
// -------------------------------------------------------------------------------------------

/// Upstream's settle for an isolated step: capture the diff, publish the manifest with the task
/// `preserved`, run the preserve-gated removal, publish again with the ledger
/// (`subagent-runner.ts:4732-4779`). The child's work survives the removal because the captured
/// patch is on disk and the manifest records it; the tree itself does NOT leak.
///
/// **Gutting mutation this fails on:** returning early from `run_single_step` before
/// `ManagedLaunch::finalize` — no manifest is written and the worktree stays on disk, which is
/// both halves of "getting cleanup wrong" at once.
#[tokio::test]
async fn an_isolated_step_publishes_its_handoff_and_does_not_leak_the_worktree() {
    let repo = clean_repo();
    let base = tempfile::tempdir().expect("base");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let manifest_path = artifacts.path().join("handoff.json");

    let graph = vec![RunnerStep::SingleStep(step(
        "isolated",
        "write something",
        WorktreeRequest::Isolated,
    ))];
    let observer = Arc::new(Observer::writing("child-output.txt"));
    let executor: Arc<dyn SingleStepExecutor> = observer.clone();
    let mut registry = OutputRegistry::new();

    let (results, _groups) = walk_chain(
        &graph,
        &mut registry,
        &executor,
        &ctx_in(repo.path(), base.path(), Some(manifest_path.clone())),
    )
    .await
    .expect("the walk succeeds");

    let worktree_cwd = observer.observed()[0].cwd.clone();
    let raw = std::fs::read_to_string(&manifest_path)
        .expect("the settle publishes a hand-off manifest beside the run");
    let manifest: serde_json::Value = serde_json::from_str(&raw).expect("manifest JSON");
    assert_eq!(manifest["mode"], "single", "{manifest:#}");
    assert_eq!(manifest["source"], "async", "{manifest:#}");
    let group = &manifest["groups"][0];
    let child = &group["children"][0];
    assert_eq!(child["agent"], "isolated", "{manifest:#}");
    assert_eq!(child["status"], "completed", "{manifest:#}");
    // Phase 1's durable capture: the child's work is a patch on disk BEFORE anything is removed.
    assert_eq!(
        child["patch"]["changed"],
        serde_json::json!(true),
        "{manifest:#}"
    );
    assert!(
        child["patch"]["diffStat"]
            .as_str()
            .is_some_and(|stat| stat.contains("child-output.txt")),
        "the captured patch must be the child's own work: {manifest:#}"
    );
    let patch_path = PathBuf::from(
        child["patch"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("a captured patch has a path: {manifest:#}")),
    );
    assert!(
        patch_path.exists(),
        "the patch file must outlive the worktree it was cut from: {patch_path:?}"
    );
    // Phase 2's ledger, and upstream's allocation id: `${runId}-s${stepIndex}`.
    let cleanup_task = &group["cleanup"]["tasks"][0];
    assert_eq!(
        cleanup_task["path"].as_str().map(PathBuf::from),
        Some(worktree_cwd.clone()),
        "the ledger must name the worktree the child actually ran in: {manifest:#}"
    );
    assert_eq!(
        cleanup_task["worktreeRemoved"],
        serde_json::json!(true),
        "a captured, published worktree is removed — preserve-then-remove, not preserve-forever: \
         {manifest:#}"
    );
    assert!(
        cleanup_task["branch"]
            .as_str()
            .is_some_and(|branch| branch.contains("suba149run-s0")),
        "the allocation is named `${{runId}}-s${{stepIndex}}` (pi `subagent-runner.ts:4443`): \
         {manifest:#}"
    );
    assert!(
        !worktree_cwd.exists(),
        "a captured, published worktree must not be left on disk"
    );
    assert!(
        results[0]
            .final_output
            .as_deref()
            .is_some_and(|text| text.contains("Worktree handoff:")),
        "the hand-off reference rides the step's output (pi `:4772`): {:?}",
        results[0].final_output
    );
}

/// A DETACHED child is still executing inside its worktree, so the settle must not run at all —
/// pi's gate is `if (singleWorktreeSetup && !singleResult.detached)`
/// (`subagent-runner.ts:4732`), the same exemption its foreground twin applies at
/// `subagent-executor.ts:4387`.
///
/// **Gutting mutation this fails on:** dropping `|| result.detached` from `run_single_step`'s
/// hand-off gate — the manifest appears and the live child's tree is removed underneath it.
#[tokio::test]
async fn a_detached_isolated_step_keeps_its_worktree_and_publishes_no_removal() {
    let repo = clean_repo();
    let base = tempfile::tempdir().expect("base");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let manifest_path = artifacts.path().join("handoff.json");

    let graph = vec![RunnerStep::SingleStep(step(
        "isolated",
        "detach mid-flight",
        WorktreeRequest::Isolated,
    ))];
    let observer = Arc::new(Observer::detaching("in-flight.txt"));
    let executor: Arc<dyn SingleStepExecutor> = observer.clone();
    let mut registry = OutputRegistry::new();

    walk_chain(
        &graph,
        &mut registry,
        &executor,
        &ctx_in(repo.path(), base.path(), Some(manifest_path.clone())),
    )
    .await
    .expect("the walk succeeds");

    let worktree_cwd = observer.observed()[0].cwd.clone();
    assert!(
        worktree_cwd.exists(),
        "a detached child is still writing here; its worktree must survive the settle"
    );
    assert!(
        worktree_cwd.join("in-flight.txt").exists(),
        "and so must its work"
    );
    assert!(
        !manifest_path.exists(),
        "a detached run publishes no removal ledger — there is nothing settled to record"
    );

    // Owned cleanup: nothing removed this tree, so the test does (the `TempDir`s drop afterwards,
    // and a registered worktree left behind would also leave a `.git/worktrees` entry).
    git_ok(
        repo.path(),
        &[
            "worktree",
            "remove",
            "--force",
            &worktree_cwd.to_string_lossy(),
        ],
    );
}

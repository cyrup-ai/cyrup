//! LANES_2 end to end: `worktree.cleanup` through the REAL `subagent` tool, against REAL git
//! worktrees and a REAL handoff manifest on disk.
//!
//! Every row drives [`cyrup_core::Tool::execute`] with a raw JSON params object — the same seam
//! both model-facing registrations call — and never a `spawn::cleanup_plan` function directly. A
//! cleanup planner that works only when called from Rust is a library, not a feature, and this
//! programme has shipped that five times.
//!
//! These live in-crate rather than in `cyrup-it` deliberately: `cyrup-it` is
//! `required-features = ["it"]` and off the `cargo test --workspace` merge gate, so a
//! reachability test that lived only there would not actually gate anything.
//! `scheduled_runs_tests.rs` is the precedent and states the same reasoning.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult};

use crate::extension::executor::SubagentExecutor;
use crate::extension::testsupport::{arm_scoped_missions, tool_text};
use crate::extension::tool::SubagentTool;

// =================================================================================================
// Fixture
// =================================================================================================

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A real repository with one commit — the same shape `spawn/worktree.rs`'s own fixture builds.
fn make_real_git_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Cleanup Plan Tests"]);
    std::fs::write(dir.join("tracked.txt"), "initial\n").expect("tracked");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "initial commit"]);
}

/// What [`fixture`] built, so a row can point assertions at it.
struct Fixture {
    _repo_dir: tempfile::TempDir,
    _base_dir: tempfile::TempDir,
    repo: PathBuf,
    base: PathBuf,
    worktree: PathBuf,
    artifacts: PathBuf,
    run_id: String,
}

/// A repository with ONE managed worktree and a handoff manifest that claims it as preserved and
/// pending cleanup.
///
/// Everything here is real: a real `git init`, a real `git worktree add`, and a manifest written
/// as literal JSON so this file also pins the on-disk spelling the plan builder reads. The only
/// thing the fixture fakes is the *provenance* of the manifest — in production it is written by
/// `handoff::write_group` at the end of a `worktree: true` fan-out (see the `[EXEC — manifest]`
/// sibling), which is why a plan against a real fan-out is empty until that writer has run.
fn fixture(child_status: &str, run_state: &str) -> Fixture {
    let repo_dir = tempfile::tempdir().expect("repo tempdir");
    let base_dir = tempfile::tempdir().expect("base tempdir");
    make_real_git_repo(repo_dir.path());
    // The realpath, because every containment check in the plan compares canonical paths and a
    // macOS `/var` -> `/private/var` symlink would otherwise make the fixture disagree with the
    // production code for a reason that has nothing to do with the feature.
    let repo = std::fs::canonicalize(repo_dir.path()).expect("repo realpath");
    let base = std::fs::canonicalize(base_dir.path()).expect("base realpath");

    let run_id = "cleanup-fixture-run";
    // The leaf name matters: containment requires the `cyrup-worktree-` prefix
    // `spawn::worktree::build_worktree_path` writes.
    let worktree = base.join(format!("cyrup-worktree-{run_id}-0"));
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "lane-0",
            &worktree.to_string_lossy(),
        ],
    );
    let worktree = std::fs::canonicalize(&worktree).expect("worktree realpath");
    let base_commit = git(&repo, &["rev-parse", "HEAD"]);

    let artifacts = crate::artifacts::project_artifacts_dir(&repo);
    std::fs::create_dir_all(&artifacts).expect("artifacts dir");

    let manifest = serde_json::json!({
        "version": 1,
        "runId": run_id,
        "mode": "parallel",
        // `async` so the owning run's terminality is proved by the status file beside the
        // manifest, which is the rung this fixture wants to exercise. A `foreground` manifest
        // would route to the injected ownership probe instead, and a run this process never
        // launched is deliberately `unknown` there — see `an_unowned_foreground_run_is_not_provably_terminal`.
        "source": "async",
        "cwd": repo,
        "createdAt": 1_750_000_000_000i64,
        "updatedAt": 1_750_000_000_000i64,
        "groups": [{
            "stepIndex": 0,
            "baseCommit": base_commit,
            "repoRoot": repo,
            "children": [{
                "index": 0,
                "taskIndex": 0,
                "agent": "worker",
                "status": child_status,
                "summary": "done",
                "patch": {
                    "path": artifacts.join("task-0.patch"),
                    "branch": "lane-0",
                    "changed": false,
                    "diffStat": "",
                    "filesChanged": 0,
                    "insertions": 0,
                    "deletions": 0
                }
            }],
            "cleanup": {
                "state": "partial",
                "pruned": false,
                "tasks": [{
                    "index": 0,
                    "path": worktree,
                    "branch": "lane-0",
                    "worktreeRemoved": false,
                    "branchRemoved": false,
                    "preserved": true,
                    // NOT one of the two reasons that mean "something still needs this": a
                    // freshly written manifest carries `cleanup pending durable handoff capture`
                    // and is Ineligible by design.
                    "reason": "ready"
                }]
            }
        }]
    });
    std::fs::write(
        artifacts.join("handoff.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");

    // The owning run's status, beside the manifest — pi `readStatusBesideManifest`.
    let mut status = serde_json::to_value(crate::background::RunStatus::queued(
        crate::background::RunId::from_token(run_id),
        crate::background::RunMode::Parallel,
        None,
    ))
    .expect("status json");
    status["state"] = serde_json::json!(run_state);
    std::fs::write(
        artifacts.join("status.json"),
        serde_json::to_vec_pretty(&status).expect("status bytes"),
    )
    .expect("write status");

    Fixture {
        _repo_dir: repo_dir,
        _base_dir: base_dir,
        repo,
        base,
        worktree,
        artifacts,
        run_id: run_id.to_string(),
    }
}

/// A tool whose executor is configured exactly as a default install plus the one config key this
/// verb reads (`subagents.worktreeBaseDir`, pi `:6229`).
async fn tool_for(fixture: &Fixture) -> SubagentTool {
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, &fixture.artifacts).await;
    executor.config_cell().lock().await.worktree_base_dir = Some(fixture.base.clone());
    SubagentTool::new(executor, fixture.repo.clone())
}

async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
    tool.execute(
        ToolCallId::from("t"),
        params,
        CancelToken::new(),
        Box::new(|_u: cyrup_core::ToolUpdate| {}),
    )
    .await
}

/// Read back the ONE plan file the dispatch persisted, through `serde` — which also exercises
/// `deny_unknown_fields` and `PlanId`'s re-validating `Deserialize`.
fn read_persisted_plan(repo: &Path) -> crate::spawn::cleanup_plan::model::WorktreeCleanupPlan {
    let dir = crate::artifacts::project_subagents_dir(repo).join("cleanup-plans");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("no plan directory at {}: {err}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 1, "exactly one plan file must be written");
    let text = std::fs::read_to_string(&files[0]).expect("plan file readable");
    serde_json::from_str(&text).expect("the persisted plan round-trips through its own type")
}

fn entry_for<'a>(
    plan: &'a crate::spawn::cleanup_plan::model::WorktreeCleanupPlan,
    worktree: &Path,
) -> &'a crate::spawn::cleanup_plan::model::CleanupPlanEntry {
    plan.entries
        .iter()
        .find(|entry| entry.path == worktree)
        .unwrap_or_else(|| {
            panic!(
                "the plan must carry an entry for {}; it has {:?}",
                worktree.display(),
                plan.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
            )
        })
}

// =================================================================================================
// Rows
// =================================================================================================

/// THE advertise-vs-dispatch invariant for this verb: it is in `SUBAGENT_ACTIONS`, so it must
/// reach a real arm and not the unknown-action fallback.
#[tokio::test]
async fn worktree_cleanup_is_advertised_and_dispatches() {
    assert!(
        crate::extension::tool::text::subagent_actions().contains(&"worktree.cleanup"),
        "the verb must be advertised"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await;
    let text = match &reply {
        Ok(result) => tool_text(result),
        Err(error) => error.to_string(),
    };
    assert!(
        !text.to_lowercase().contains("unknown subagent action"),
        "worktree.cleanup is advertised but lands on the unknown-action arm: {text}"
    );
}

/// **The load-bearing row.** A worktree holding UNTRACKED work must never be proposed for removal.
///
/// It is negative and positive on the same run: the entry must appear (so an empty plan, a
/// deleted classifier or a refused verb fails it) AND its verdict must be `dirty/keep` (so
/// returning `remove` for everything, or dropping the `git status` gate, fails it). Untracked
/// rather than modified on purpose — that is the case `--untracked-files=all` exists for, and the
/// one git's default `normal` mode can summarize away.
#[tokio::test]
async fn a_worktree_with_untracked_work_is_never_proposed_for_removal() {
    let fixture = fixture("completed", "complete");
    std::fs::create_dir_all(fixture.worktree.join("fresh")).expect("fresh dir");
    std::fs::write(fixture.worktree.join("fresh/new.txt"), "unsaved work\n").expect("untracked");
    // `status.showUntrackedFiles = no` is an ordinary operator setting — plenty of people carry it
    // in `~/.gitconfig` on large repositories — and it makes a BARE `git status --porcelain=v1`
    // print nothing at all while the worktree holds unsaved work. Setting it here is what makes
    // this row pin the explicit `--untracked-files=all` flag rather than merely exercise it:
    // without the flag, this fixture reads clean and the entry goes Safe/Remove.
    git(
        &fixture.worktree,
        &["config", "status.showUntrackedFiles", "no"],
    );
    assert!(
        git(&fixture.worktree, &["status", "--porcelain=v1"]).is_empty(),
        "the fixture is only meaningful if the DEFAULT status reads clean"
    );

    let tool = tool_for(&fixture).await;
    let result = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await
    .expect("the plan builds");
    let text = tool_text(&result);

    assert!(
        text.contains("worktree has uncommitted or untracked changes"),
        "the operator must be told WHY the worktree is kept:\n{text}"
    );
    let kept_section = text
        .split("Will keep, with reasons")
        .nth(1)
        .expect("the render carries a keep section");
    assert!(
        kept_section.contains(&fixture.worktree.to_string_lossy().to_string()),
        "the dirty worktree must be listed under 'Will keep':\n{text}"
    );
    let will_remove = text
        .split("Will remove")
        .nth(1)
        .and_then(|rest| rest.split("Will delete local branches").next())
        .expect("the render carries a remove section");
    assert!(
        !will_remove.contains(&fixture.worktree.to_string_lossy().to_string()),
        "a dirty worktree must NEVER appear under 'Will remove':\n{text}"
    );

    let plan = read_persisted_plan(&fixture.repo);
    let entry = entry_for(&plan, &fixture.worktree);
    assert_eq!(
        entry.state,
        crate::spawn::cleanup_plan::model::CleanupState::Dirty
    );
    assert_eq!(
        entry.decision,
        crate::spawn::cleanup_plan::model::CleanupDecision::Keep
    );
    assert_eq!(
        entry.will_delete_branch, None,
        "a kept entry must carry no branch-deletion flag at all"
    );
    // The digest, not the filenames: the plan file lives inside the operator's repository.
    assert!(
        entry.preconditions.status_digest.is_some(),
        "the dirty status must still be digest-recorded for a later apply phase"
    );
    let raw = std::fs::read_to_string(
        crate::artifacts::project_subagents_dir(&fixture.repo)
            .join("cleanup-plans")
            .join(format!("{}.json", plan.plan_id)),
    )
    .expect("plan file");
    assert!(
        !raw.contains("new.txt"),
        "the plan must not leak the operator's untracked filenames"
    );
}

/// The positive control for the row above: with the untracked file gone, the SAME fixture is
/// proposed for removal. Without this, "never propose removal" would be satisfiable by proposing
/// nothing, ever.
#[tokio::test]
async fn a_clean_preserved_worktree_is_proposed_for_removal() {
    let fixture = fixture("completed", "complete");
    let tool = tool_for(&fixture).await;
    let result = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await
    .expect("the plan builds");
    let text = tool_text(&result);

    let plan = read_persisted_plan(&fixture.repo);
    let entry = entry_for(&plan, &fixture.worktree);
    assert_eq!(
        entry.state,
        crate::spawn::cleanup_plan::model::CleanupState::Safe,
        "a clean, terminal, contained, metadata-claimed worktree is removable:\n{text}"
    );
    assert_eq!(
        entry.decision,
        crate::spawn::cleanup_plan::model::CleanupDecision::Remove
    );
    // The branch tip IS the target HEAD here, so nothing is lost by deleting it.
    assert_eq!(entry.will_delete_branch, Some(true));
    assert_eq!(entry.run_id.as_deref(), Some(fixture.run_id.as_str()));
    assert_eq!(entry.task_index, Some(0));
    assert!(entry.preconditions.branch_tip.is_some());
    assert!(entry.preconditions.base_commit.is_some());

    // Plan-only, and the render says so in both places a human could misread.
    assert!(text.contains("Plan-only mode: no worktrees or branches were removed."));
    assert!(text.contains(&format!(
        "Plan saved: {}",
        plan_path(&fixture, &plan).display()
    )));

    // And nothing was actually removed.
    assert!(
        fixture.worktree.exists(),
        "building a plan must not remove the worktree it plans against"
    );
    assert!(
        git(&fixture.repo, &["branch", "--list", "lane-0"]).contains("lane-0"),
        "building a plan must not delete the branch"
    );
}

fn plan_path(
    fixture: &Fixture,
    plan: &crate::spawn::cleanup_plan::model::WorktreeCleanupPlan,
) -> PathBuf {
    crate::spawn::cleanup_plan::worktree_cleanup_plan_path(&fixture.repo, &plan.plan_id)
}

/// An owning run that has not settled keeps its worktree, whatever the filesystem says.
#[tokio::test]
async fn an_active_owning_run_keeps_its_worktree() {
    let fixture = fixture("completed", "running");
    let tool = tool_for(&fixture).await;
    dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await
    .expect("the plan builds");

    let plan = read_persisted_plan(&fixture.repo);
    let entry = entry_for(&plan, &fixture.worktree);
    assert_eq!(
        entry.state,
        crate::spawn::cleanup_plan::model::CleanupState::Active
    );
    assert_eq!(
        entry.decision,
        crate::spawn::cleanup_plan::model::CleanupDecision::Keep
    );
    assert!(entry.reasons.iter().any(|r| r == "owning run is running"));
}

/// A settled run whose CHILD never settled is still active — pi `:493`. This is the rung that
/// stops a detached child's worktree being removed out from under it.
#[tokio::test]
async fn a_non_terminal_child_keeps_the_worktree() {
    let fixture = fixture("detached", "complete");
    let tool = tool_for(&fixture).await;
    dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await
    .expect("the plan builds");

    let plan = read_persisted_plan(&fixture.repo);
    let entry = entry_for(&plan, &fixture.worktree);
    assert_eq!(
        entry.state,
        crate::spawn::cleanup_plan::model::CleanupState::Active,
        "`detached` is deliberately NOT a terminal child status"
    );
    assert!(
        entry
            .reasons
            .iter()
            .any(|r| r == "owning handoff still has a non-terminal child")
    );
}

/// A worktree outside the managed base directory is never removable, even when everything else
/// about it is perfect. The base dir is where this build puts its own worktrees; anything else is
/// the operator's.
#[tokio::test]
async fn a_worktree_outside_the_managed_base_dir_is_never_removable() {
    let fixture = fixture("completed", "complete");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    // Point the config at a directory that contains nothing, so the real worktree is out of it.
    let tool = {
        let executor = Arc::new(SubagentExecutor::new());
        arm_scoped_missions(&executor, &fixture.artifacts).await;
        executor.config_cell().lock().await.worktree_base_dir =
            Some(std::fs::canonicalize(elsewhere.path()).expect("elsewhere realpath"));
        SubagentTool::new(executor, fixture.repo.clone())
    };
    dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await
    .expect("the plan builds");

    let plan = read_persisted_plan(&fixture.repo);
    let entry = entry_for(&plan, &fixture.worktree);
    assert_eq!(
        entry.state,
        crate::spawn::cleanup_plan::model::CleanupState::Ineligible
    );
    assert_eq!(
        entry.decision,
        crate::spawn::cleanup_plan::model::CleanupDecision::Keep
    );
    assert!(
        entry
            .reasons
            .iter()
            .any(|r| r.contains("outside configured base directory"))
    );
}

/// A FOREGROUND manifest whose run this process never launched is `unknown`, never `terminal` —
/// the injected ownership probe's whole reason for being required rather than optional.
#[tokio::test]
async fn an_unowned_foreground_run_is_not_provably_terminal() {
    let fixture = fixture("completed", "complete");
    // Flip the manifest to `foreground` and drop the status file, so the only remaining proof of
    // termination would have to come from the probe.
    let manifest_path = fixture.artifacts.join("handoff.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("manifest json");
    manifest["source"] = serde_json::json!("foreground");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("bytes"),
    )
    .expect("rewrite manifest");
    std::fs::remove_file(fixture.artifacts.join("status.json")).expect("drop status");

    let tool = tool_for(&fixture).await;
    dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
    )
    .await
    .expect("the plan builds");

    let plan = read_persisted_plan(&fixture.repo);
    let entry = entry_for(&plan, &fixture.worktree);
    assert_eq!(
        entry.state,
        crate::spawn::cleanup_plan::model::CleanupState::Unknown
    );
    assert_eq!(
        entry.decision,
        crate::spawn::cleanup_plan::model::CleanupDecision::Unknown
    );
    assert!(
        entry
            .reasons
            .iter()
            .any(|r| r == "foreground owning-run state is not provably terminal")
    );
}

/// The two refusals that ARE the "a stale plan must never be executed" guard, since no apply
/// phase exists anywhere — upstream included. Both sentences are pinned verbatim.
#[tokio::test]
async fn apply_mode_and_plan_id_are_refused() {
    let fixture = fixture("completed", "complete");
    let tool = tool_for(&fixture).await;

    let apply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "apply" }),
    )
    .await
    .expect_err("apply must be refused");
    assert_eq!(
        apply.to_string(),
        "worktree.cleanup currently supports mode='plan' only; apply/removal is not available yet."
    );

    let with_plan_id = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan", "planId": "abc" }),
    )
    .await
    .expect_err("planId must be refused");
    assert_eq!(
        with_plan_id.to_string(),
        "worktree.cleanup plan mode does not accept planId; apply is not available yet."
    );

    // A refused call writes nothing.
    assert!(
        !crate::artifacts::project_subagents_dir(&fixture.repo)
            .join("cleanup-plans")
            .exists(),
        "a refused dispatch must not persist a plan"
    );
}

/// The child-safe gate, checked BEFORE `mode` and before `planId` — upstream's own order
/// (`:6214` above `:6217` above `:6220`). Reordering would leak the shape of the refusal, and
/// through it the fact that this repository holds preserved worktrees, to a caller not permitted
/// to invoke the verb at all.
#[tokio::test]
async fn child_safe_fanout_refuses_worktree_cleanup_before_validating_anything() {
    let fixture = fixture("completed", "complete");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, &fixture.artifacts).await;
    let child_safe = SubagentTool::new_child_safe(executor, fixture.repo.clone());

    for params in [
        serde_json::json!({ "action": "worktree.cleanup", "mode": "plan" }),
        // Would be refused for `mode` first if the gate were ordered wrongly.
        serde_json::json!({ "action": "worktree.cleanup", "mode": "apply" }),
        serde_json::json!({ "action": "worktree.cleanup" }),
    ] {
        let refused = dispatch(&child_safe, params)
            .await
            .expect_err("a child-safe tool must refuse this verb");
        assert_eq!(
            refused.to_string(),
            "Action 'worktree.cleanup' is not available from child-safe subagent fanout mode."
        );
    }
}

/// `repo` and `handoffPath` are resolved against the REQUEST cwd, and a relative `repo` reaches
/// the right repository. Also the proof that the two properties are read at all: advertising a
/// property the dispatch drops is the defect `schema.rs`'s own guard exists to catch.
#[tokio::test]
async fn repo_and_handoff_path_are_read_and_resolved_against_the_request_cwd() {
    let fixture = fixture("completed", "complete");
    let parent = fixture
        .repo
        .parent()
        .expect("repo has a parent")
        .to_path_buf();
    let leaf = fixture
        .repo
        .file_name()
        .expect("repo has a leaf")
        .to_string_lossy()
        .into_owned();

    // The tool's cwd is the repo's PARENT, so a bare dispatch would plan for the wrong directory.
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, &fixture.artifacts).await;
    executor.config_cell().lock().await.worktree_base_dir = Some(fixture.base.clone());
    let tool = SubagentTool::new(executor, parent);

    let result = dispatch(
        &tool,
        serde_json::json!({
            "action": "worktree.cleanup",
            "mode": "plan",
            "repo": leaf,
            "handoffPath": fixture.artifacts.join("handoff.json"),
        }),
    )
    .await
    .expect("a relative repo resolves against the request cwd");

    let plan = read_persisted_plan(&fixture.repo);
    assert_eq!(plan.repo_root, fixture.repo);
    assert_eq!(
        entry_for(&plan, &fixture.worktree).state,
        crate::spawn::cleanup_plan::model::CleanupState::Safe
    );
    assert!(tool_text(&result).contains(&fixture.repo.display().to_string()));
}

/// A manifest the caller NAMED that cannot be read is warned about; one merely discovered is not
/// (pi `:429`). The warning reaches the operator through the render.
#[tokio::test]
async fn an_explicitly_named_unreadable_manifest_is_warned_about() {
    let fixture = fixture("completed", "complete");
    let tool = tool_for(&fixture).await;
    let missing = fixture.artifacts.join("not-a-manifest.json");

    let result = dispatch(
        &tool,
        serde_json::json!({
            "action": "worktree.cleanup",
            "mode": "plan",
            "handoffPath": missing,
        }),
    )
    .await
    .expect("an unreadable named manifest is a warning, not a failure");
    let text = tool_text(&result);
    assert!(text.contains("Warnings"), "{text}");
    assert!(
        text.contains("parallel handoff manifest not found"),
        "{text}"
    );
    // And the discovered manifest still produced its entry, so one bad path did not lose the
    // whole analysis.
    let plan = read_persisted_plan(&fixture.repo);
    assert_eq!(
        entry_for(&plan, &fixture.worktree).state,
        crate::spawn::cleanup_plan::model::CleanupState::Safe
    );
}

/// A plan id read back off disk goes through `PlanId::parse`, so a hand-edited traversal id is
/// refused at deserialization rather than being joined into a path.
#[test]
fn a_traversing_plan_id_does_not_deserialize() {
    for hostile in ["../../etc/passwd", "..", ".", "a/b", ""] {
        let raw = serde_json::json!(hostile);
        assert!(
            serde_json::from_value::<crate::spawn::cleanup_plan::model::PlanId>(raw).is_err(),
            "plan id {hostile:?} must not deserialize"
        );
    }
    assert!(
        serde_json::from_value::<crate::spawn::cleanup_plan::model::PlanId>(serde_json::json!(
            "a.b-c_1"
        ))
        .is_ok()
    );
}

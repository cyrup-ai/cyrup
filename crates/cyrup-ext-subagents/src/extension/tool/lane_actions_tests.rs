//! LANES_2 end to end: `worktree.discard`, `lane.status`, `lane.recordMerge` and
//! `lane.recordSupersession` through the REAL `subagent` tool, against a REAL git worktree and a
//! REAL handoff manifest on disk.
//!
//! Every row drives [`cyrup_core::Tool::execute`] with a raw JSON params object — the same seam
//! both model-facing registrations call — and **no row constructs a `LaneAction`, a
//! `handoff::Manifest` or a `DiscardAuthorization` directly**. A verb that only works when called
//! from Rust is a library, not a feature.
//!
//! In-crate rather than in `cyrup-it` for the reason `worktree_cleanup_tests.rs` states: `cyrup-it`
//! is `required-features = ["it"]` and off the `cargo test --workspace` merge gate, so a
//! reachability test that lived only there would not gate anything.
//!
//! The manifest fixture is written as literal JSON rather than through `handoff::write_group`, on
//! purpose: it also pins the on-disk spelling these verbs read, and it keeps the rows about THIS
//! area's dispatch rather than about the fan-out that produces a manifest (which the
//! `[EXEC — manifest]` sibling proves in `cyrup-it`).

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
use crate::registration::authority::{AuthorityDecision, AuthorityPolicyConfig};

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

fn git_status(cwd: &Path, args: &[&str]) -> bool {
    StdCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git runs")
        .status
        .success()
}

fn make_real_git_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Lane Action Tests"]);
    std::fs::write(dir.join("tracked.txt"), "initial\n").expect("tracked");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "initial commit"]);
}

const RUN_ID: &str = "lane-fixture-run";
const LANE_BRANCH: &str = "lane-0";
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    _repo_dir: tempfile::TempDir,
    _base_dir: tempfile::TempDir,
    repo: PathBuf,
    worktree: PathBuf,
    manifest: PathBuf,
}

/// A repository with ONE real managed worktree and a manifest that records it as preserved, with
/// one TERMINAL child — the shape a settled fan-out leaves behind.
fn fixture(child_status: &str) -> Fixture {
    let repo_dir = tempfile::tempdir().expect("repo tempdir");
    let base_dir = tempfile::tempdir().expect("base tempdir");
    make_real_git_repo(repo_dir.path());
    let repo = std::fs::canonicalize(repo_dir.path()).expect("repo realpath");
    let base = std::fs::canonicalize(base_dir.path()).expect("base realpath");

    let worktree = base.join(format!("cyrup-worktree-{RUN_ID}-0"));
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            LANE_BRANCH,
            &worktree.to_string_lossy(),
        ],
    );
    let worktree = std::fs::canonicalize(&worktree).expect("worktree realpath");
    let base_commit = git(&repo, &["rev-parse", "HEAD"]);

    let artifacts = crate::artifacts::project_artifacts_dir(&repo);
    std::fs::create_dir_all(&artifacts).expect("artifacts dir");
    let manifest_path = artifacts.join("handoff.json");

    let manifest = serde_json::json!({
        "version": 1,
        "runId": RUN_ID,
        "mode": "parallel",
        // `foreground`, so the plan/status machinery does not need a status.json beside it —
        // these rows are about the lane verbs, not about run-ownership inference.
        "source": "foreground",
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
                // The launch-declared lane, in the exact on-disk spelling
                // `crate::workflows::WorkflowLaneMetadata` produces.
                "lane": {
                    "version": 1,
                    "key": "lane.a",
                    "mode": "mutation",
                    "sourceRef": "main",
                    "claims": ["src/a.rs"],
                    "outputPaths": ["out/a.json"]
                },
                "patch": {
                    "path": artifacts.join("task-0.patch"),
                    "branch": LANE_BRANCH,
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
                    "branch": LANE_BRANCH,
                    // LANES_2 — the allocator and its naming evidence, the two keys
                    // `deny_unknown_fields` used to make this file unreadable.
                    "provider": "native",
                    "naming": {
                        "requestedBranch": LANE_BRANCH,
                        "branchPrefix": "lane-",
                        "label": "lane a",
                        "sanitizedPathComponent": "lane-a"
                    },
                    "worktreeRemoved": false,
                    "branchRemoved": false,
                    "preserved": true,
                    "reason": "ready"
                }]
            }
        }]
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");

    Fixture {
        _repo_dir: repo_dir,
        _base_dir: base_dir,
        repo,
        worktree,
        manifest: manifest_path,
    }
}

async fn tool_for(fixture: &Fixture) -> SubagentTool {
    let executor = Arc::new(SubagentExecutor::new());
    let artifacts = crate::artifacts::project_artifacts_dir(&fixture.repo);
    arm_scoped_missions(&executor, &artifacts).await;
    SubagentTool::new(executor, fixture.repo.clone())
}

/// A tool whose authority policy answers `decision` for `discardWorktree`.
async fn tool_with_policy(fixture: &Fixture, decision: AuthorityDecision) -> SubagentTool {
    let tool = tool_for(fixture).await;
    tool.executor().config_cell().lock().await.authority_policy = Some(AuthorityPolicyConfig {
        discard_worktree: Some(decision),
        ..Default::default()
    });
    tool
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

fn text_of(reply: &Result<ToolResult, ToolError>) -> String {
    match reply {
        Ok(result) => tool_text(result),
        Err(error) => error.to_string(),
    }
}

fn valid_merge() -> serde_json::Value {
    serde_json::json!({
        "prNumber": 42,
        "reviewedHead": SHA_A,
        "mergeCommit": SHA_B,
        "treeEquivalent": true,
        "postMergeChecks": "recorded",
        "attestedBy": "reviewer",
        "attestedAt": "2026-09-18T00:00:00Z"
    })
}

// =================================================================================================
// Advertise-vs-dispatch
// =================================================================================================

/// Every verb this change advertises must reach a real arm, not the unknown-action fallback — the
/// invariant the crate has three tests against, asserted here through the live dispatch.
#[tokio::test]
async fn every_lane_verb_is_advertised_and_dispatches() {
    let advertised = crate::extension::tool::text::subagent_actions();
    for verb in [
        "worktree.discard",
        "worktree.cleanup",
        "lane.status",
        "lane.recordMerge",
        "lane.recordSupersession",
    ] {
        assert!(advertised.contains(&verb), "{verb} must be advertised");
    }

    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    // Deliberately underspecified calls: each must land on its OWN refusal, which proves the arm
    // ran, and never on "unknown subagent action".
    for verb in [
        "worktree.discard",
        "lane.status",
        "lane.recordMerge",
        "lane.recordSupersession",
    ] {
        let text = text_of(&dispatch(&tool, serde_json::json!({ "action": verb })).await);
        assert!(
            !text.to_lowercase().contains("unknown subagent action"),
            "{verb} is advertised but lands on the unknown-action arm: {text}"
        );
    }
}

// =================================================================================================
// lane.status — the read-only verb
// =================================================================================================

/// `lane.status` renders the stored manifest's eligibility through the real dispatch — and a
/// manifest that carries no `cleanupEligibility` key at all reads as **unknown**, i.e. not safe.
///
/// That is the fail-safe default and it is upstream's: a freshly written manifest deliberately has
/// no such key (`parallel-handoff.ts:602-608`), and `trustedStoredCleanupEligibility` (`:313`)
/// treats an absent stored verdict the same as a forged one. The verb never invents permission.
#[tokio::test]
async fn lane_status_reads_the_manifest_through_the_tool() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.status",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
        }),
    )
    .await;
    let text = text_of(&reply);
    assert!(reply.is_ok(), "{text}");
    assert!(
        text.contains("Cleanup eligibility: unknown") && text.contains("removal is not safe"),
        "a manifest with no recorded verdict is never eligible: {text}"
    );
    assert!(
        text.contains("worktree.cleanup")
            && text.contains(&fixture.manifest.to_string_lossy().to_string()),
        "the render must hand back a plan command naming THIS manifest: {text}"
    );
}

/// **The anti-forgery rule, through the real verb.** A hand-edited `cleanupEligibility` claiming
/// `terminal-eligible` must not be believed: the verdict is re-derived from the evidence, and a
/// disagreement collapses to `unknown` — which reads as "removal is not safe".
///
/// Without this, editing one string in a JSON file would be enough to talk the cleanup planner
/// into proposing a worktree for removal.
#[tokio::test]
async fn a_forged_stored_eligibility_is_not_believed() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.manifest).expect("manifest"))
            .expect("manifest json");
    stored["cleanupEligibility"] = serde_json::json!({ "state": "terminal-eligible" });
    std::fs::write(
        &fixture.manifest,
        serde_json::to_vec_pretty(&stored).expect("bytes"),
    )
    .expect("rewrite");

    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.status",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
        }),
    )
    .await;
    let text = text_of(&reply);
    assert!(
        text.contains("Cleanup eligibility: unknown"),
        "a forged verdict must collapse to unknown, not be echoed back: {text}"
    );
    assert!(!text.contains("terminal-eligible"), "{text}");
}

/// pi `:6283` — a lane that does not name the manifest's own run is an ERROR, not prose.
#[tokio::test]
async fn lane_status_refuses_a_lane_that_is_not_this_manifests_run() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.status",
            "laneId": "some-other-run",
            "handoffPath": fixture.manifest,
        }),
    )
    .await;
    assert!(reply.is_err(), "a run mismatch is an error");
    assert!(
        text_of(&reply)
            .contains("Lane 'some-other-run' does not match manifest run 'lane-fixture-run'."),
        "{}",
        text_of(&reply)
    );
}

/// pi `:6282`'s bare `catch` — and ONLY there. A missing manifest renders as "removal is not
/// safe", because a read failure must never be mistaken for permission to remove.
#[tokio::test]
async fn lane_status_renders_a_missing_manifest_as_not_safe() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.status",
            "laneId": RUN_ID,
            "handoffPath": fixture.repo.join("nope.json"),
        }),
    )
    .await;
    let text = text_of(&reply);
    assert!(
        reply.is_ok(),
        "an unreadable manifest is prose, not an error"
    );
    assert!(
        text.contains("Cleanup eligibility: unknown") && text.contains("removal is not safe"),
        "{text}"
    );
}

// =================================================================================================
// The child-safe split — the reason this feature has an enum
// =================================================================================================

/// **The load-bearing negative/positive pair.** A child-safe fanout tool may READ its lane graph
/// and may not write to it. Both halves on one fixture, so "refuse everything" cannot pass.
#[tokio::test]
async fn a_child_safe_fanout_may_read_its_lane_graph_but_not_record_evidence() {
    let fixture = fixture("completed");
    let executor = Arc::new(SubagentExecutor::new());
    let artifacts = crate::artifacts::project_artifacts_dir(&fixture.repo);
    arm_scoped_missions(&executor, &artifacts).await;
    let child = SubagentTool::new_child_safe(executor, fixture.repo.clone());

    // (a) the read SUCCEEDS.
    let status = dispatch(
        &child,
        serde_json::json!({
            "action": "lane.status",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
        }),
    )
    .await;
    assert!(
        status.is_ok(),
        "lane.status must stay available to a child-safe fanout: {}",
        text_of(&status)
    );
    assert!(text_of(&status).contains("Cleanup eligibility:"));

    // (b) every mutating verb is refused, with pi's exact sentence, and the gate runs BEFORE the
    // param validation — note that none of these calls carries `laneId` or `handoffPath`, so a
    // dispatch that validated first would answer with a DIFFERENT sentence.
    for verb in [
        "lane.recordMerge",
        "lane.recordSupersession",
        "worktree.discard",
        "worktree.cleanup",
    ] {
        let reply = dispatch(&child, serde_json::json!({ "action": verb })).await;
        assert!(reply.is_err(), "{verb} must be refused");
        assert_eq!(
            text_of(&reply),
            format!("Action '{verb}' is not available from child-safe subagent fanout mode."),
            "{verb}'s refusal must be pi's verbatim sentence, checked before any param"
        );
    }
}

// =================================================================================================
// lane.recordMerge / lane.recordSupersession
// =================================================================================================

/// The convergence path, end to end: record merge evidence through the tool, see the manifest
/// become eligible, and see `details.parallelHandoff` on the result.
///
/// That last assertion is the one that revives two landed, currently-dead production readers —
/// `missions/lifecycle.rs`'s `artifacts_for_result` and `sync_mission_from_async_completion` both
/// read `details.parallelHandoff.path` and file a `MissionArtifactKind::Manifest`.
#[tokio::test]
async fn record_merge_makes_the_lane_eligible_and_reports_the_manifest() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;

    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.recordMerge",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
            "merge": valid_merge(),
        }),
    )
    .await;
    let result = reply.expect("valid merge evidence is recorded");
    let details = result.details.clone().expect("management details");
    assert_eq!(details["mode"], "management");
    assert_eq!(details["results"], serde_json::json!([]));
    assert_eq!(
        details["parallelHandoff"]["path"],
        serde_json::json!(fixture.manifest),
        "the result must carry the manifest reference missions/lifecycle.rs reads"
    );

    // The stored eligibility, re-read through the verb rather than off the struct.
    let after = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.status",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
        }),
    )
    .await;
    assert!(
        text_of(&after).contains("Cleanup eligibility: terminal-eligible"),
        "{}",
        text_of(&after)
    );
}

/// Evidence that does not attest tree equivalence must NOT make a lane eligible — the blocker is
/// named so an operator can act on it.
#[tokio::test]
async fn merge_evidence_without_tree_equivalence_blocks_rather_than_clears() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let mut merge = valid_merge();
    merge["treeEquivalent"] = serde_json::json!(false);

    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.recordMerge",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
            "merge": merge,
        }),
    )
    .await;
    let text = text_of(&reply);
    assert!(
        text.contains("terminal-blocked")
            && text.contains("merged tree is not equivalent to the reviewed head"),
        "{text}"
    );
}

/// A lane with a NON-terminal child is still active, and evidence is refused — the rule that
/// keeps an attestation from being recorded while the work it attests to is still running.
#[tokio::test]
async fn evidence_is_refused_while_a_child_is_still_running() {
    let fixture = fixture("running");
    let tool = tool_for(&fixture).await;
    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.recordMerge",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
            "merge": valid_merge(),
        }),
    )
    .await;
    assert!(reply.is_err());
    assert!(
        text_of(&reply).contains("Lane has an active child owner"),
        "{}",
        text_of(&reply)
    );
}

/// The two missing-parameter refusals, in upstream's own wording, naming the verb.
#[tokio::test]
async fn the_lane_verbs_name_themselves_in_their_missing_parameter_refusals() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;

    let no_lane = dispatch(
        &tool,
        serde_json::json!({ "action": "lane.recordMerge", "handoffPath": fixture.manifest }),
    )
    .await;
    assert_eq!(text_of(&no_lane), "lane.recordMerge requires laneId.");

    let no_path = dispatch(
        &tool,
        serde_json::json!({ "action": "lane.status", "laneId": RUN_ID }),
    )
    .await;
    assert_eq!(
        text_of(&no_path),
        "lane.status requires handoffPath for the existing parallel handoff manifest."
    );

    // A BLANK path is "absent", not "the cwd" — the distinction the raw-String params carry.
    let blank = dispatch(
        &tool,
        serde_json::json!({ "action": "lane.status", "laneId": RUN_ID, "handoffPath": "   " }),
    )
    .await;
    assert_eq!(
        text_of(&blank),
        "lane.status requires handoffPath for the existing parallel handoff manifest."
    );
}

/// A lane cannot supersede itself, and the refusal comes from the recorder's own sentence.
#[tokio::test]
async fn a_lane_cannot_supersede_itself_through_the_tool() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let reply = dispatch(
        &tool,
        serde_json::json!({
            "action": "lane.recordSupersession",
            "laneId": RUN_ID,
            "handoffPath": fixture.manifest,
            "supersession": {
                "supersededBy": RUN_ID,
                "attestedBy": "reviewer",
                "attestedAt": "2026-09-18T00:00:00Z"
            },
        }),
    )
    .await;
    assert!(reply.is_err());
    assert!(
        text_of(&reply)
            .contains("supersession.supersededBy must identify a different replacement lane."),
        "{}",
        text_of(&reply)
    );
}

// =================================================================================================
// worktree.discard — the destructive verb
// =================================================================================================

/// **The positive control for the destructive path.** With the policy set to `auto`, the verb
/// really removes a real worktree and a real branch, and the manifest's ledger says so.
///
/// Paired with the refusal rows below, this is what stops "never discard anything" from passing.
#[tokio::test]
async fn an_authorized_discard_removes_the_real_worktree_and_branch() {
    let fixture = fixture("completed");
    let tool = tool_with_policy(&fixture, AuthorityDecision::Auto).await;
    assert!(fixture.worktree.exists(), "fixture precondition");

    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    let text = text_of(&reply);
    assert!(reply.is_ok(), "{text}");
    assert!(
        text.contains("Discard processed 1 preserved worktree."),
        "{text}"
    );
    assert!(
        !fixture.worktree.exists(),
        "the worktree directory must actually be gone"
    );
    assert!(
        !git_status(&fixture.repo, &["rev-parse", "--verify", LANE_BRANCH]),
        "the temporary branch must actually be deleted"
    );

    // The ledger, read back off disk as JSON — the on-disk spelling is the interface.
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.manifest).expect("manifest"))
            .expect("manifest json");
    let task = &stored["groups"][0]["cleanup"]["tasks"][0];
    assert_eq!(task["worktreeRemoved"], serde_json::json!(true));
    assert_eq!(task["branchRemoved"], serde_json::json!(true));
    assert!(
        task.get("preserved").is_none(),
        "a fully removed task must not still claim to be preserved: {task}"
    );
    assert_eq!(stored["groups"][0]["cleanup"]["state"], "complete");
}

/// `forbid` refuses with pi's verb-SPECIFIC sentence — not the generic
/// `Authority policy forbids action '<x>'.` — and changes nothing.
#[tokio::test]
async fn a_forbidden_discard_refuses_and_touches_nothing() {
    let fixture = fixture("completed");
    let tool = tool_with_policy(&fixture, AuthorityDecision::Forbid).await;
    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    assert!(reply.is_err());
    assert_eq!(
        text_of(&reply),
        "Authority policy forbids worktree discard."
    );
    assert!(fixture.worktree.exists(), "nothing may be removed");
}

/// **The default install prompts, and a prompt with nobody to ask is a REFUSAL.**
///
/// No policy is configured here at all: `resolve_authority_decision` falls back to
/// `default_decision`, which is `Confirm` for `discardWorktree`. A test executor has no host
/// services, so there is no UI — and an absent UI must never read as an implicit yes.
#[tokio::test]
async fn the_default_policy_confirms_and_a_session_with_no_ui_refuses() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    assert!(
        tool.executor()
            .config_snapshot()
            .await
            .authority_policy
            .is_none(),
        "this row is about the DEFAULT, so no policy may be configured"
    );

    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    assert!(reply.is_err());
    assert_eq!(
        text_of(&reply),
        "Authority policy requires user confirmation for worktree discard, but this session has \
         no interactive UI. Preserved worktrees were not changed."
    );
    assert!(fixture.worktree.exists(), "nothing may be removed");
}

/// **Declining is a CHOICE, not a failure.** pi `:6258` carries no `isError`; returning `Err`
/// would make a deliberate "no" look like a transient failure to an orchestrator retry loop,
/// which would then re-prompt the user.
#[tokio::test]
async fn declining_the_confirm_is_ok_not_an_error() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    tool.executor()
        .set_host_services(Arc::new(cyrup_ext::host::RecordingServices::new(
            cyrup_ext::host::CannedResponses {
                confirm: false,
                ..Default::default()
            },
        )));

    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    assert!(reply.is_ok(), "a decline is Ok: {}", text_of(&reply));
    assert_eq!(
        text_of(&reply),
        "Worktree discard canceled; preserved worktrees were not changed."
    );
    assert!(fixture.worktree.exists(), "nothing may be removed");
}

/// The confirm body must name the manifest — "discard preserved worktrees?" is not a question
/// anyone can answer without knowing which ones — and a yes actually removes them.
#[tokio::test]
async fn a_confirmed_discard_names_the_manifest_in_its_prompt_and_then_removes() {
    let fixture = fixture("completed");
    let tool = tool_for(&fixture).await;
    let services = Arc::new(cyrup_ext::host::RecordingServices::new(
        cyrup_ext::host::CannedResponses {
            confirm: true,
            ..Default::default()
        },
    ));
    tool.executor().set_host_services(services.clone());

    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    assert!(reply.is_ok(), "{}", text_of(&reply));
    let prompts = services.confirm_messages();
    assert_eq!(prompts.len(), 1, "exactly one confirm: {prompts:?}");
    assert!(
        prompts[0].contains(&fixture.manifest.to_string_lossy().to_string()),
        "the confirm body must name the manifest: {}",
        prompts[0]
    );
    assert!(!fixture.worktree.exists());
}

/// **The second safety gate, reached through the real verb.** Before removing anything under a
/// `discard` intent, `cleanup_worktrees` probes the worktree with `git status --porcelain` and a
/// base-commit diff (pi `worktree.ts:1171-1192`). When the probe cannot answer — here because the
/// directory was removed out from under the manifest — the task is PRESERVED with pi's
/// `cleanup safety check failed`, never removed on the assumption that there was nothing in it.
///
/// This is also what keeps the gate from being defence-in-depth nobody can reach: the
/// authorization arm below it only refuses for callers other than this dispatch, but the probe arm
/// is reachable from an ordinary stale manifest.
#[tokio::test]
async fn a_worktree_the_manifest_can_no_longer_inspect_is_preserved_not_removed() {
    let fixture = fixture("completed");
    let tool = tool_with_policy(&fixture, AuthorityDecision::Auto).await;
    // The manifest still claims this worktree; the directory is gone.
    std::fs::remove_dir_all(&fixture.worktree).expect("remove the worktree directory");

    let reply = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    let text = text_of(&reply);
    assert!(reply.is_ok(), "{text}");
    assert!(
        text.contains("Some worktrees remain. Inspect and remove them manually if appropriate:")
            && text.contains("worktree remove --force"),
        "an operator must be handed the manual commands: {text}"
    );

    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.manifest).expect("manifest"))
            .expect("manifest json");
    let task = &stored["groups"][0]["cleanup"]["tasks"][0];
    assert_eq!(task["preserved"], serde_json::json!(true));
    assert_eq!(task["reason"], "cleanup safety check failed");
    assert_eq!(task["worktreeRemoved"], serde_json::json!(false));
    assert_eq!(stored["groups"][0]["cleanup"]["state"], "partial");
    assert!(
        git_status(&fixture.repo, &["rev-parse", "--verify", LANE_BRANCH]),
        "the branch must survive a removal that was refused"
    );
}

/// A missing `handoffPath` gets the verb's OWN sentence, which names where to find one.
#[tokio::test]
async fn discard_without_a_handoff_path_says_where_to_get_one() {
    let fixture = fixture("completed");
    let tool = tool_with_policy(&fixture, AuthorityDecision::Auto).await;
    let reply = dispatch(&tool, serde_json::json!({ "action": "worktree.discard" })).await;
    assert_eq!(
        text_of(&reply),
        "worktree.discard requires handoffPath from parallelHandoff.path or async status."
    );
}

/// A discard against a manifest that is already fully cleaned up is a no-op that says so, rather
/// than a second round of `git worktree remove` against paths that no longer exist.
#[tokio::test]
async fn a_second_discard_is_a_no_op_that_says_so() {
    let fixture = fixture("completed");
    let tool = tool_with_policy(&fixture, AuthorityDecision::Auto).await;
    let first = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    assert!(first.is_ok(), "{}", text_of(&first));

    let second = dispatch(
        &tool,
        serde_json::json!({ "action": "worktree.discard", "handoffPath": fixture.manifest }),
    )
    .await;
    assert!(
        text_of(&second).contains("No preserved worktrees remain in"),
        "{}",
        text_of(&second)
    );
}

// =================================================================================================
// The lane property, launch side
// =================================================================================================

/// The `lane` launch param is validated at the dispatch boundary with the NORMALIZER's own
/// sentence — pi `:3805-3808` answers an invalid lane with the message, not with a schema
/// rejection a model cannot act on.
#[tokio::test]
async fn an_invalid_launch_lane_is_refused_with_the_normalizers_sentence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let reply = dispatch(
        &tool,
        serde_json::json!({
            "tasks": [{ "agent": "worker", "task": "t" }],
            "lane": { "version": 1, "key": "lane.a", "mode": "sideways" },
        }),
    )
    .await;
    assert_eq!(text_of(&reply), "lane.mode is invalid.");

    let unknown = dispatch(
        &tool,
        serde_json::json!({
            "tasks": [{ "agent": "worker", "task": "t" }],
            "lane": { "version": 1, "key": "lane.a", "whoops": 1 },
        }),
    )
    .await;
    assert_eq!(text_of(&unknown), "lane has unsupported fields: whoops.");
}

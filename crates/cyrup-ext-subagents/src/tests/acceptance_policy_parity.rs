//! SUBA-C13 — an authored `acceptance` policy's `criteria` / `evidence` / `review` / `stopRules`
//! must reach the child prompt AND the gate, not merely pass validation.
//!
//! Ports the two upstream halves the live path used to drop on the floor
//! (`pi-subagents/src/runs/shared/acceptance.ts` @v0.34.0):
//!
//! - `formatAcceptancePrompt` (`:304-348`, applied at `runs/foreground/execution.ts:1037` and
//!   `runs/background/subagent-runner.ts:871`) — the `- <id>: <must>` criteria list, the
//!   `Required evidence: <kinds>` line, the review-gate line, the stop rules and the exact
//!   `acceptance-report` JSON template.
//! - `evaluateAcceptance`'s `rank >= checked` rung (`:819-829`) — `checkCriteriaSatisfied`
//!   (`:622-630`) fails any required criterion the child did not report as `satisfied`;
//!   `runStructuralChecks` (`:657-669`) fails any declared evidence kind missing from the child
//!   report and shells out to a REAL `git status --short` for `no-staged-files` (`:646-655`).
//!
//! Before this change `lower_acceptance_input` validated all four keys and then lowered only
//! `level` + `verify[]`, so `{ level: "checked", criteria: [...], evidence: [...] }` armed a gate
//! that could never fire: the child was never told the criteria, and a report carrying neither the
//! criteria nor the evidence still reached `Checked` purely because the completion-mutation guard
//! had not triggered.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::exec::acceptance::{
    AcceptanceStatus, CleanCompletionGate, evaluate_acceptance, inject_acceptance_contract,
    lower_acceptance_input,
};
use crate::exec::completion_guard::CompletionMutationGuardResult;

/// The declared policy from the backlog item's own impact statement, plus a review gate and a stop
/// rule so all four keys are exercised by one fixture.
fn policy() -> serde_json::Value {
    serde_json::json!({
        "level": "checked",
        "criteria": [
            { "id": "c1", "must": "add a regression test" },
            "keep the public API stable"
        ],
        "evidence": ["tests-added", "residual-risks"],
        "review": { "agent": "reviewer", "focus": "public API surface", "required": false },
        "stopRules": ["stop if the fix needs a schema migration"]
    })
}

/// A finished run that is clean by every other measure: exit 0, not detached/interrupted/timed out.
fn clean_gate() -> CleanCompletionGate {
    CleanCompletionGate {
        exit_code: 0,
        detached: false,
        interrupted: false,
        timed_out: false,
    }
}

/// The completion-mutation guard did NOT fire — pre-fix, this alone was enough to reach `Checked`.
fn guard_did_not_fire() -> CompletionMutationGuardResult {
    CompletionMutationGuardResult {
        expected_mutation: true,
        attempted_mutation: true,
        triggered: false,
    }
}

// ================================================================================================
// Injection half — `formatAcceptancePrompt` (acceptance.ts:403-457)
// ================================================================================================

#[test]
fn declared_criteria_evidence_review_and_stop_rules_reach_the_child_prompt() {
    let contract = lower_acceptance_input(&policy())
        .expect("the policy is valid per validateAcceptanceInput")
        .expect("a `level` was declared, so a contract is lowered");

    let prompt = inject_acceptance_contract("Fix the bug", &contract);

    assert!(prompt.starts_with("Fix the bug"), "task text comes first: {prompt}");
    // `- ${criterion.id}: ${criterion.must}` (acceptance.ts:417).
    assert!(
        prompt.contains("- c1: add a regression test"),
        "the explicit criterion id/must line is missing: {prompt}"
    );
    // A bare `must` string gets the `criterion-<n>` id from `normalizeCriteria` (acceptance.ts:332).
    assert!(
        prompt.contains("- criterion-2: keep the public API stable"),
        "the bare-string criterion is missing its normalized id: {prompt}"
    );
    // `Required evidence: ${acceptance.evidence.join(", ") || "none"}` (acceptance.ts:419).
    assert!(
        prompt.contains("Required evidence: tests-added, residual-risks"),
        "the required-evidence line is missing: {prompt}"
    );
    // `Review gate: ${... required === false ? "optional" : "required"}${... by <agent>}.`
    // (acceptance.ts:321-323).
    assert!(
        prompt.contains("Review gate: optional by reviewer."),
        "the review gate line is missing: {prompt}"
    );
    assert!(
        prompt.contains("Review focus: public API surface"),
        "the review focus line is missing: {prompt}"
    );
    // `Stop rules:` + `- ${rule}` (acceptance.ts:325-327).
    assert!(
        prompt.contains("Stop rules:")
            && prompt.contains("- stop if the fix needs a schema migration"),
        "the stop rules are missing: {prompt}"
    );
    // The report template pi emits verbatim (acceptance.ts:333-344).
    assert!(
        prompt.contains("```acceptance-report") && prompt.contains("\"criteriaSatisfied\""),
        "the acceptance-report template is missing: {prompt}"
    );
}

// ================================================================================================
// Enforcement half — `evaluateAcceptance`'s `rank >= checked` rung (acceptance.ts:1297-1321)
// ================================================================================================

/// A child report that claims success but reports NEITHER declared criterion and carries NEITHER
/// declared evidence kind. Pre-fix this reached `Checked`; pi rejects it.
const BARE_REPORT: &str = "Done.\n```acceptance-report\n{\"criteriaSatisfied\": []}\n```\n";

#[tokio::test]
async fn a_child_report_missing_the_declared_criteria_and_evidence_is_rejected() {
    let contract = lower_acceptance_input(&policy())
        .expect("valid policy")
        .expect("a contract is lowered");
    assert_eq!(contract.required_level, AcceptanceStatus::Checked, "premise");

    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = evaluate_acceptance(
        &contract,
        clean_gate(),
        Some(BARE_REPORT),
        guard_did_not_fire(),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "a report satisfying none of the declared policy must not reach checked: {ledger:?}"
    );
    let detail = ledger.detail.unwrap_or_default();
    // `checkCriteriaSatisfied`'s verbatim message (acceptance.ts:915).
    assert!(
        detail.contains("Required criterion 'c1' was not reported."),
        "pi's criterion message is missing: {detail}"
    );
    // `runStructuralChecks`'s verbatim message (acceptance.ts:956).
    assert!(
        detail.contains("tests-added evidence missing from child report."),
        "pi's evidence message is missing: {detail}"
    );
}

#[tokio::test]
async fn a_child_report_satisfying_the_declared_policy_reaches_checked() {
    let contract = lower_acceptance_input(&policy())
        .expect("valid policy")
        .expect("a contract is lowered");

    // Both criteria reported `satisfied`, plus the two declared evidence kinds. `residualRisks`
    // counts as present whenever the key is a string array at all (`reportEvidencePresent`,
    // acceptance.ts:637), so an empty array is legitimate "no residual risks".
    let report = "All done.\n```acceptance-report\n{\
        \"criteriaSatisfied\": [\
          {\"id\": \"c1\", \"status\": \"satisfied\", \"evidence\": \"tests/foo.rs\"},\
          {\"id\": \"criterion-2\", \"status\": \"satisfied\", \"evidence\": \"no API change\"}\
        ],\
        \"testsAddedOrUpdated\": [\"tests/foo.rs\"],\
        \"residualRisks\": []\
    }\n```\n";

    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = evaluate_acceptance(
        &contract,
        clean_gate(),
        Some(report),
        guard_did_not_fire(),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(
        ledger.status,
        AcceptanceStatus::Checked,
        "every declared criterion and evidence kind is present: {ledger:?}"
    );
}

#[tokio::test]
async fn a_criterion_reported_as_not_satisfied_is_rejected_with_pis_wording() {
    let contract = lower_acceptance_input(&serde_json::json!({
        "level": "checked",
        "criteria": [{ "id": "c1", "must": "add a regression test" }]
    }))
    .expect("valid policy")
    .expect("a contract is lowered");

    let report = "```acceptance-report\n{\"criteriaSatisfied\": [\
        {\"id\": \"c1\", \"status\": \"not-satisfied\", \"evidence\": \"ran out of time\"}]}\n```";

    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = evaluate_acceptance(
        &contract,
        clean_gate(),
        Some(report),
        guard_did_not_fire(),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(ledger.status, AcceptanceStatus::Rejected);
    assert!(
        ledger
            .detail
            .unwrap_or_default()
            .contains("Required criterion 'c1' was reported as not-satisfied."),
        "pi's `checkCriteriaSatisfied` not-satisfied message must survive onto the ledger"
    );
}

/// `severity: "recommended"` criteria are FILTERED OUT of the gate (`acceptance.ts:624`) — they
/// still appear in the prompt, but never reject.
#[tokio::test]
async fn a_recommended_criterion_is_prompted_but_never_gates() {
    let contract = lower_acceptance_input(&serde_json::json!({
        "level": "checked",
        "criteria": [{ "id": "nice", "must": "update the changelog", "severity": "recommended" }]
    }))
    .expect("valid policy")
    .expect("a contract is lowered");

    assert!(
        inject_acceptance_contract("Fix it", &contract).contains("- nice: update the changelog"),
        "a recommended criterion is still shown to the child"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = evaluate_acceptance(
        &contract,
        clean_gate(),
        Some("```acceptance-report\n{\"criteriaSatisfied\": []}\n```"),
        guard_did_not_fire(),
        dir.path(),
        None,
        None,
    )
    .await;

    assert_eq!(
        ledger.status,
        AcceptanceStatus::Checked,
        "a recommended criterion must not reject: {ledger:?}"
    );
}

/// `no-staged-files` is the one evidence kind backed by an orchestrator-observed fact rather than
/// the child's own claim: `checkNoStagedFiles` runs a REAL `git status --short` in the run cwd
/// (acceptance.ts:939-948) and fails when anything is staged, no matter what the child reported.
#[tokio::test]
async fn declared_no_staged_files_evidence_runs_a_real_git_status_and_rejects_a_staged_worktree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    };
    let Ok(status) = git(&["init", "--quiet"]) else {
        eprintln!("git is unavailable; skipping the real-subprocess half of this test");
        return;
    };
    assert!(status.success(), "git init failed");
    std::fs::write(cwd.join("staged.txt"), b"content").expect("write");
    assert!(git(&["add", "staged.txt"]).expect("git add").success());

    let contract = lower_acceptance_input(&serde_json::json!({
        "level": "checked",
        "evidence": ["no-staged-files"]
    }))
    .expect("valid policy")
    .expect("a contract is lowered");

    // The child CLAIMS a clean index. The orchestrator's own `git status` says otherwise, and the
    // orchestrator wins (DI-SA-5).
    let report = "```acceptance-report\n{\"criteriaSatisfied\": [], \"noStagedFiles\": true}\n```";
    let ledger = evaluate_acceptance(
        &contract,
        clean_gate(),
        Some(report),
        guard_did_not_fire(),
        cwd,
        None,
        None,
    )
    .await;

    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "a staged worktree must reject even though the child self-reported `noStagedFiles: true`: \
         {ledger:?}"
    );
    assert!(
        ledger
            .detail
            .unwrap_or_default()
            .contains("Staged files present: A  staged.txt"),
        "pi's `checkNoStagedFiles` message must name the staged path"
    );
}

/// An object with no `level` and no `verify[]` is pi's `level: "auto"`: the level is inferred, but
/// the declared policy still resolves (`resolveEffectiveAcceptance`, acceptance.ts:344-401). This
/// arm used to return `None` and throw the whole policy away.
#[test]
fn a_policy_with_no_level_still_lowers_and_carries_its_criteria() {
    let contract = lower_acceptance_input(&serde_json::json!({
        "criteria": [{ "id": "c1", "must": "add a regression test" }]
    }))
    .expect("valid policy")
    .expect("a level-less policy is still a policy");

    assert_eq!(contract.criteria.len(), 1);
    assert_eq!(contract.criteria[0].id, "c1");
    assert!(
        contract.explicit,
        "pi's `explicit` is `input.explicit !== undefined`, which this object satisfies"
    );
}

//! SUBA-082 — the DECLARED-role branch of `inferLevel`
//! (`pi-subagents:v0.57.0:src/runs/shared/acceptance.ts:90-104`; `:92-108` @v0.64.0), a direct
//! mirror of upstream's own case list, `test/unit/acceptance.test.ts:91-165` @v0.64.0 ("uses
//! explicit agent roles for ambiguous tasks while preserving task-intent precedence").
//!
//! What the role changes, and what it must NOT change:
//!
//! - `readOnlyAgent` is `role === "read-only" || (role === undefined && /\b(?:reviewer|…)\b/)`
//!   and `writeTask` gains `role === "writer" && !readOnlyTask` — so a declared role REPLACES the
//!   agent-name guess (`explorer` + `read-only` → read-only branch; `reviewer` + `writer` → checked).
//! - task-intent precedence is preserved: an implementation task on a `read-only` worker is still
//!   `checked`, and `Review only; do not edit files` on a `writer` is still read-only.
//! - with a role declared, `Patch src/auth.ts` counts as mutation (`rolePatchTask`) even though
//!   the classifier alone calls it `unknown`.
//! - a declared role that resolves read-only cancels the `dynamic`/`dynamicGroup` escalation
//!   (`roleResolvesReadOnly`) and the risky-keyword escalation (`keywordRiskReadOnly`).
//!
//! One deliberate difference from the v0.64.0 assertions: upstream's read-only branch resolves to
//! level `none` there, this crate's to `attested`. That is v0.63.0's `0128385f` ("fix: omit
//! inferred acceptance for read-only reviewers (#1799)"), a separate drift that also rewrites the
//! NAME-classified branch and is not part of the acceptance-role row — see `infer_level`'s doc.
//! Every case below therefore asserts the BRANCH (`checked` vs. the read-only branch's
//! `review-findings` evidence and reason string), which is what the role decides on both tags.
//!
//! Everything here goes through `model::resolve_effective_acceptance`, the same function
//! `AcceptanceContract::heuristic_default_for_role` calls on the live inference path.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::exec::acceptance::model::{
    AcceptanceEvidenceKind, AcceptanceLevel, AcceptanceResolveInput, AcceptanceRole,
    ResolvedAcceptanceConfig, resolve_effective_acceptance,
};
use crate::exec::acceptance::{AcceptanceContract, AcceptanceStatus};

fn infer(agent: &str, role: Option<AcceptanceRole>, task: &str) -> ResolvedAcceptanceConfig {
    resolve_effective_acceptance(&AcceptanceResolveInput {
        explicit: None,
        agent_name: agent.to_string(),
        acceptance_role: role,
        task: Some(task.to_string()),
        mode: None,
        is_async: false,
        dynamic: false,
        dynamic_group: false,
    })
}

/// The read-only branch's own evidence set (`acceptance.ts:135-137` @v0.57.0) — the observable
/// that separates it from BOTH `checked` and the default fallthrough.
const READ_ONLY_BRANCH_EVIDENCE: [AcceptanceEvidenceKind; 2] = [
    AcceptanceEvidenceKind::ReviewFindings,
    AcceptanceEvidenceKind::ResidualRisks,
];

fn assert_read_only_branch(resolved: &ResolvedAcceptanceConfig, reason: &str, case: &str) {
    assert_ne!(
        resolved.level,
        AcceptanceLevel::Checked,
        "{case}: must not be checked"
    );
    assert_eq!(
        resolved.evidence, READ_ONLY_BRANCH_EVIDENCE,
        "{case}: not the read-only branch"
    );
    assert_eq!(resolved.inferred_reason, vec![reason.to_string()], "{case}");
}

/// `explorer` is outside every name alternation, so on ambiguous wording it falls through to the
/// default attestation — UNLESS it declares `read-only`, which takes the read-only branch with
/// the role reason (`acceptance.ts:133` @v0.57.0: `"declared read-only acceptance role"`).
#[test]
fn a_declared_read_only_role_replaces_the_agent_name_guess() {
    let control = infer("explorer", None, "Explore the authentication flow");
    assert_eq!(
        control.inferred_reason,
        vec!["default lightweight attestation".to_string()],
        "control: without a role the name decides, and `explorer` is in no alternation"
    );
    assert_read_only_branch(
        &infer(
            "explorer",
            Some(AcceptanceRole::ReadOnly),
            "Explore the authentication flow",
        ),
        "declared read-only acceptance role",
        "explorer + read-only",
    );
    // `worker` + `read-only` on neutral wording: the `\bworker\b` name arm is gated on
    // `role === undefined`, so the declared role wins.
    assert_read_only_branch(
        &infer(
            "worker",
            Some(AcceptanceRole::ReadOnly),
            "Explore the authentication flow",
        ),
        "declared read-only acceptance role",
        "worker + read-only",
    );
    assert_read_only_branch(
        &infer("worker", Some(AcceptanceRole::ReadOnly), "Create a report"),
        "declared read-only acceptance role",
        "worker + read-only + read-only deliverable",
    );
}

/// `reviewer` + `writer`: the reviewer-name alternation is gated on `role === undefined`, and the
/// `writer` arm of `writeTask` fires on any non-read-only task — with the role's own reason
/// (`acceptance.ts:124` @v0.57.0: `"declared writer acceptance role"`, because `taskMayWrite` is
/// false for this wording).
#[test]
fn a_declared_writer_role_replaces_the_reviewer_name_guess() {
    let control = infer("reviewer", None, "Handle the authentication flow");
    assert_read_only_branch(
        &control,
        "read-only/reviewer-style agent",
        "control: reviewer name",
    );

    let writer = infer(
        "reviewer",
        Some(AcceptanceRole::Writer),
        "Handle the authentication flow",
    );
    assert_eq!(writer.level, AcceptanceLevel::Checked);
    assert_eq!(
        writer.inferred_reason,
        vec!["declared writer acceptance role".to_string()]
    );

    // With genuine mutation wording the reason is the ordinary one — the role is not what made
    // the task write-capable.
    let implementing = infer(
        "reviewer",
        Some(AcceptanceRole::Writer),
        "Implement the fix",
    );
    assert_eq!(implementing.level, AcceptanceLevel::Checked);
    assert_eq!(
        implementing.inferred_reason,
        vec!["write-capable worker/task".to_string()]
    );

    // `async` + a role-declared writer is the risky branch, exactly as for a name-classified
    // worker (`acceptance.ts:106`).
    let async_writer = resolve_effective_acceptance(&AcceptanceResolveInput {
        agent_name: "reviewer".to_string(),
        acceptance_role: Some(AcceptanceRole::Writer),
        task: Some("Handle the authentication flow".to_string()),
        is_async: true,
        ..Default::default()
    });
    assert_eq!(async_writer.level, AcceptanceLevel::Checked);
    assert_eq!(
        async_writer.inferred_reason,
        vec!["async write-capable or risky run".to_string()]
    );
}

/// Task-intent precedence: explicit mutation wording beats a declared `read-only` role
/// (`taskMayWrite` is computed on the `worker` grammar when a role is declared,
/// `acceptance.ts:90,97`), including `Patch src/auth.ts`, which only `rolePatchTask` catches.
#[test]
fn explicit_mutation_intent_wins_over_a_declared_read_only_role() {
    for task in [
        "Implement the authentication fix",
        "Create a fixture",
        "Add coverage",
        "Replace the dependency",
        "Patch src/auth.ts",
    ] {
        let resolved = infer("worker", Some(AcceptanceRole::ReadOnly), task);
        assert_eq!(resolved.level, AcceptanceLevel::Checked, "{task}");
        assert_eq!(
            resolved.inferred_reason,
            vec!["write-capable worker/task".to_string()],
            "{task}"
        );
    }
    // The control for `rolePatchTask`: WITHOUT a role, `Patch src/auth.ts` is `unknown` to the
    // classifier and a worker-named agent only reaches `checked` through the `\bworker\b` arm —
    // so a name outside every alternation stays on the default fallthrough.
    assert_eq!(
        infer("explorer", None, "Patch src/auth.ts").inferred_reason,
        vec!["default lightweight attestation".to_string()],
        "control: `rolePatchTask` is gated on a declared role"
    );
    // Async + read-only role + patch wording: risky, because `writeTask` is true.
    let async_patch = resolve_effective_acceptance(&AcceptanceResolveInput {
        agent_name: "worker".to_string(),
        acceptance_role: Some(AcceptanceRole::ReadOnly),
        task: Some("Patch src/auth.ts".to_string()),
        is_async: true,
        ..Default::default()
    });
    assert_eq!(async_patch.level, AcceptanceLevel::Checked);
}

/// The mirror: explicit no-edit wording beats a declared `writer` role (`readOnlyTask` short-
/// circuits `taskMayWrite` and gates the `writer` arm, `acceptance.ts:97,101`), on the single
/// path and under a `dynamicGroup` escalation alike.
#[test]
fn explicit_no_edit_wording_wins_over_a_declared_writer_role() {
    assert_read_only_branch(
        &infer(
            "worker",
            Some(AcceptanceRole::Writer),
            "Review only; do not edit files",
        ),
        "read-only task wording",
        "worker + writer + review-only wording",
    );
    let dynamic_group = resolve_effective_acceptance(&AcceptanceResolveInput {
        agent_name: "worker".to_string(),
        acceptance_role: Some(AcceptanceRole::Writer),
        task: Some("Review only; do not edit files".to_string()),
        dynamic_group: true,
        ..Default::default()
    });
    assert_ne!(
        dynamic_group.level,
        AcceptanceLevel::Checked,
        "`roleResolvesReadOnly` cancels the dynamicGroup escalation for a declared role"
    );
}

/// With a role declared, `keywordRiskReadOnly` is `inferredReadOnly` rather than the bare
/// classifier verdict (`acceptance.ts:105`), so `security` in an explorer's read-only task no
/// longer escalates; without a role the same wording on a worker still does (upstream's own
/// "preserves risky keyword review inference when acceptance role metadata is omitted" case).
#[test]
fn a_declared_read_only_role_suppresses_the_risky_keyword_escalation() {
    assert_read_only_branch(
        &infer(
            "explorer",
            Some(AcceptanceRole::ReadOnly),
            "Audit the security posture",
        ),
        "declared read-only acceptance role",
        "explorer + read-only + security keyword",
    );
    for task in ["Inspect the security posture", "Read-only security audit"] {
        let no_role = infer("worker", None, task);
        assert_eq!(no_role.level, AcceptanceLevel::Checked, "{task}");
        assert_eq!(
            no_role.inferred_reason,
            vec!["risky write-capable run".to_string()],
            "{task}: the `undefined` branch keeps the keyword escalation"
        );
    }
}

/// `roleResolvesReadOnly` cancels the `dynamic` escalation (`acceptance.ts:107` @v0.57.0):
/// `explorer` + `read-only` on `Explore each target` under dynamic fan-out is NOT risky.
/// Without a role the very same input escalates to `checked` — the control that shows the
/// guard is measuring the role and not the wording.
#[test]
fn a_declared_read_only_role_cancels_the_dynamic_escalation() {
    let with_role = resolve_effective_acceptance(&AcceptanceResolveInput {
        agent_name: "explorer".to_string(),
        acceptance_role: Some(AcceptanceRole::ReadOnly),
        task: Some("Explore each target".to_string()),
        dynamic: true,
        ..Default::default()
    });
    assert_read_only_branch(
        &with_role,
        "declared read-only acceptance role",
        "explorer + read-only + dynamic",
    );
    let without_role = resolve_effective_acceptance(&AcceptanceResolveInput {
        agent_name: "explorer".to_string(),
        acceptance_role: None,
        task: Some("Explore each target".to_string()),
        dynamic: true,
        ..Default::default()
    });
    assert_eq!(
        without_role.level,
        AcceptanceLevel::Checked,
        "control: with no role the dynamic escalation fires (`roleResolvesReadOnly` is false)"
    );
    assert_eq!(
        without_role.inferred_reason,
        vec![
            "risky write-capable run".to_string(),
            "dynamic fanout context".to_string()
        ]
    );
}

/// The enum-lattice entry points carry the role through unchanged, and the two-argument forms
/// are exactly the `None` role (the branch every pre-existing caller was on).
#[test]
fn the_lattice_contract_entry_points_thread_the_role() {
    assert_eq!(
        AcceptanceContract::heuristic_default("reviewer", "Handle the authentication flow"),
        AcceptanceContract::heuristic_default_for_role(
            "reviewer",
            None,
            "Handle the authentication flow"
        ),
    );
    assert_eq!(
        AcceptanceContract::heuristic_default_for_role(
            "reviewer",
            Some(AcceptanceRole::Writer),
            "Handle the authentication flow"
        )
        .required_level,
        AcceptanceStatus::Checked
    );
    assert_eq!(
        AcceptanceContract::heuristic_default_for_role(
            "worker",
            Some(AcceptanceRole::ReadOnly),
            "Explore the authentication flow"
        )
        .required_level,
        AcceptanceStatus::Attested
    );
    // The explicit-floor rule is untouched: an explicit `attested` still loses to a role-inferred
    // `checked` by rank.
    let effective = AcceptanceContract::resolve_effective_for_role(
        Some(AcceptanceContract::explicit_floor(
            AcceptanceStatus::Attested,
            Vec::new(),
        )),
        "reviewer",
        Some(AcceptanceRole::Writer),
        "Handle the authentication flow",
    );
    assert_eq!(effective.required_level, AcceptanceStatus::Checked);
    assert!(effective.explicit);
}

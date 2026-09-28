//! SUBA-108 — the DECLARED-role branch of `inferLevel` (`src/runs/shared/acceptance.ts:81-122`
//! @v0.71.0). At this tag the declared role is the ONLY input: `inferLevel` reads nothing but
//! `input.acceptanceRole` and the `async`/`dynamic`/`dynamicGroup` flags.
//!
//! This file was filed as SUBA-082, against `:90-104` @v0.57.0, where the role interacted with a
//! dozen name and wording heuristics. Upstream `7c98a696` ("refactor: remove inferred no-edit
//! completion failures", #2356, v0.70.1) deleted all of them, and with them five of this module's
//! seven tests — each of which pinned an interaction that no longer exists:
//!
//! * `a_declared_writer_role_replaces_the_reviewer_name_guess` — its control asserted that the
//!   `reviewer` NAME takes the read-only branch. There is no name alternation any more.
//! * `explicit_mutation_intent_wins_over_a_declared_read_only_role` and
//!   `explicit_no_edit_wording_wins_over_a_declared_writer_role` — task wording no longer
//!   overrides, or even reaches, the declared role. This is the deletion's whole point: CHANGELOG
//!   0.70.1, *"Stop guessing whether task wording requires file edits."*
//! * `a_declared_read_only_role_suppresses_the_risky_keyword_escalation` — the
//!   `release|migration|security|…` pattern is gone, so there is no escalation to suppress.
//! * `a_declared_read_only_role_cancels_the_dynamic_escalation` — its control asserted that a
//!   NO-role dynamic run escalates. Dynamic now escalates only for a declared `writer`
//!   (`acceptance.ts:88`), so `roleResolvesReadOnly` has no counterpart either.
//!
//! Those are deletions of tests for deleted behaviour, not suppressions. The property that
//! REPLACES them — the inferred contract depends on the role/async/dynamic axes and on nothing
//! else — is pinned directly, and more strongly than any of the five did, by
//! `exec::acceptance::model::level`'s `inference_is_invariant_across_task_text_and_agent_name` and
//! `exec::acceptance::lattice::contract`'s `the_inferred_contract_does_not_depend_on_the_agent_name`.
//! The same deletion removed this crate's `tests/read_only_agent_name_alternation.rs` outright:
//! that whole module existed to pin the `reviewer|oracle|scout|researcher|analyst` alternation.
//!
//! What survives here is the two statements that are still true: a declared `read-only` role takes
//! the read-only branch where the bare agent name never would, and the enum-lattice entry points
//! thread the role through to the contract unchanged.

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

/// The read-only branch's observable at v0.71.0: level `none` (`acceptance.ts:110`) with
/// criteria and evidence CLEARED on the way out (`:521-522`). The branch's own
/// `[review-findings, residual-risks]` list — which `:135-137` @v0.57.0 did ship — never reaches
/// the resolved config, because nothing shows a contract to a child that has no acceptance
/// prompt. The reasons survive (`:520`), which is what still separates this branch from the
/// `{ level: "none", reason }` deliberate disable.
fn assert_read_only_branch(resolved: &ResolvedAcceptanceConfig, reason: &str, case: &str) {
    assert_eq!(
        resolved.level,
        AcceptanceLevel::None,
        "{case}: the read-only branch infers `none`"
    );
    assert!(
        resolved.criteria.is_empty(),
        "{case}: level `none` clears criteria, got {:?}",
        resolved.criteria
    );
    assert!(
        resolved.evidence.is_empty(),
        "{case}: level `none` clears evidence, got {:?}",
        resolved.evidence
    );
    assert_eq!(resolved.inferred_reason, vec![reason.to_string()], "{case}");
}

/// The read-only branch's own evidence set, as `inferLevel` names it before
/// `resolveEffectiveAcceptance` discards it. It is observable only once `:505` has upgraded the
/// level away from `none`, which is what [`a_caller_supplied_policy_reaches_a_read_only_child`]
/// asserts.
const READ_ONLY_BRANCH_EVIDENCE: [AcceptanceEvidenceKind; 2] = [
    AcceptanceEvidenceKind::ReviewFindings,
    AcceptanceEvidenceKind::ResidualRisks,
];

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
        // SUBA-108 — `acceptance.ts:109-117` @v0.71.0: a declared read-only role infers `none`,
        // which lowers to `NotRequired` (no acceptance prompt, no gate).
        AcceptanceStatus::NotRequired
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

/// SUBA-108 — `acceptance.ts:505` @v0.71.0. A declared read-only agent infers `none`, and `none`
/// is where the caller's own policy goes to die: `explicitLevel` is `auto` whenever the explicit
/// input names no `level`, so the MAX escalation at `:507` never runs and the resolved level stays
/// `none` — empty contract, `NotRequired` gate, criteria/evidence/verify/review discarded by
/// `:521-522`. Upstream checks `explicitAcceptanceRequestsPolicy` first and upgrades to `attested`.
#[test]
fn a_caller_supplied_policy_reaches_a_read_only_child() {
    let bare = infer("scout", Some(AcceptanceRole::ReadOnly), "Audit the flow");
    assert_eq!(
        bare.level,
        AcceptanceLevel::None,
        "premise: no policy asked"
    );

    let upgraded = resolve_effective_acceptance(&AcceptanceResolveInput {
        explicit: Some(crate::exec::acceptance::model::AcceptanceInput::Config(
            crate::exec::acceptance::model::AcceptanceConfig {
                stop_rules: Some(vec!["do not edit".to_string()]),
                ..Default::default()
            },
        )),
        agent_name: "scout".to_string(),
        acceptance_role: Some(AcceptanceRole::ReadOnly),
        task: Some("Audit the flow".to_string()),
        mode: None,
        is_async: false,
        dynamic: false,
        dynamic_group: false,
    });
    assert_eq!(upgraded.level, AcceptanceLevel::Attested);
    assert_eq!(upgraded.stop_rules, vec!["do not edit".to_string()]);
    // `:509` compares against the UPGRADED level, so what survives is the read-only branch's own
    // evidence list, not `requiredEvidenceForLevel("attested")`.
    assert_eq!(upgraded.evidence, READ_ONLY_BRANCH_EVIDENCE);
    assert!(
        !upgraded.criteria.is_empty(),
        "the level is no longer `none`, so `:521` no longer clears the criteria"
    );
}

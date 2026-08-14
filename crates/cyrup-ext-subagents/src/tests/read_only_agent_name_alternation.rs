//! Integration test: the read-only-AGENT alternation inside `inferLevel`
//! (`pi-subagents:v0.43.0:src/runs/shared/acceptance.ts:98-99`):
//!
//! ```text
//! const readOnlyAgent = input.acceptanceRole === "read-only"
//!     || (input.acceptanceRole === undefined && /\b(?:reviewer|oracle|scout|researcher|analyst)\b/.test(agent));
//! ```
//!
//! Upstream `83b9872` ("fix: remove stale bundled roles") rewrote that alternation in ONE edit —
//! dropping `context-builder` and adding `oracle` — and cyrup applied both halves together. Neither
//! half had a test. `git log -S` over the alternation returns that one commit and no other, so this
//! file pins the post-edit membership, not a snapshot of an intermediate state.
//!
//! Why the two halves need DIFFERENT assertions, and why the obvious one is not enough:
//!
//! - `analyst` is in the alternation and in nothing else — no `isResearchAgent` match, no
//!   `isReviewerStyleAgent` match — so its membership is visible in the LEVEL, CRITERIA and EVIDENCE
//!   the contract carries. Removing it flips a neutral-worded run from the read-only branch
//!   (`review-findings`) to the default fallthrough (`manual-notes`).
//! - `oracle` is ALSO matched by `isReviewerStyleAgent` (`task-intent.ts:138-140`), which makes
//!   `classifyTaskMutationIntent` return `read-only` for it independently. Two independent
//!   mechanisms therefore agree on its level, and its alternation membership shows up in exactly one
//!   place: WHICH reason `inferLevel` records — `"read-only/reviewer-style agent"` (the agent branch,
//!   `acceptance.ts:107-109`) versus `"read-only task wording"`. That is why deleting `"oracle"`
//!   from the alternation left the whole crate suite green.
//!
//! Everything here goes through `model::resolve_effective_acceptance`, the same function
//! `AcceptanceContract::heuristic_default` calls on the live inference path — no reimplementation of
//! the classifier, and no private item reached around.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::exec::acceptance::model::{
    AcceptanceEvidenceKind, AcceptanceLevel, AcceptanceResolveInput, ResolvedAcceptanceConfig,
    resolve_effective_acceptance,
};

/// The five names the alternation contains at v0.43.0, in upstream's own order.
const READ_ONLY_AGENT_NAMES: [&str; 5] = ["reviewer", "oracle", "scout", "researcher", "analyst"];

/// Wording with no read-only keyword, no risky keyword, and no implementation verb, so the verdict
/// is decided by the AGENT NAME and nothing else.
const NEUTRAL_TASK: &str = "Weigh the two candidate approaches";

fn infer(agent: &str, task: &str) -> ResolvedAcceptanceConfig {
    resolve_effective_acceptance(&AcceptanceResolveInput {
        explicit: None,
        agent_name: agent.to_string(),
        task: Some(task.to_string()),
        mode: None,
        is_async: false,
        dynamic: false,
        dynamic_group: false,
    })
}

/// Every name in the alternation must take the read-only-AGENT branch on neutral task wording, and
/// must record the AGENT reason rather than the task-wording one.
///
/// The reason string is the only readout that separates the agent branch from the task branch for
/// the four names (`reviewer`, `oracle`, `scout`, `researcher`) that a second classifier also calls
/// read-only, so it is asserted for all five rather than only where it is uniquely load-bearing.
#[test]
fn every_read_only_agent_name_takes_the_agent_branch_of_infer_level() {
    for name in READ_ONLY_AGENT_NAMES {
        let resolved = infer(name, NEUTRAL_TASK);
        assert_eq!(
            resolved.level,
            AcceptanceLevel::Attested,
            "{name}: a read-only agent on neutral wording infers `attested`"
        );
        assert_eq!(
            resolved.inferred_reason,
            vec!["read-only/reviewer-style agent".to_string()],
            "{name}: the reason must name the AGENT branch — this is the ONLY observable that \
             distinguishes alternation membership for a name a second classifier also calls \
             read-only (`oracle` above all)"
        );
        assert_eq!(
            resolved.evidence,
            vec![
                AcceptanceEvidenceKind::ReviewFindings,
                AcceptanceEvidenceKind::ResidualRisks,
            ],
            "{name}: the read-only branch asks for review findings, not manual notes"
        );
    }
}

/// The control that proves the assertion above is measuring the ALTERNATION and not a constant: a
/// name outside it, given read-only WORDING, reaches the same branch by the other route and records
/// the other reason.
#[test]
fn a_name_outside_the_alternation_records_the_task_wording_reason_instead() {
    let resolved = infer("helper", "Inspect the config and report what you find");
    assert_eq!(resolved.level, AcceptanceLevel::Attested);
    assert_eq!(
        resolved.inferred_reason,
        vec!["read-only task wording".to_string()],
        "an agent name outside the alternation must reach the read-only branch by WORDING, and say so"
    );
}

/// `analyst` is the one alternation entry no other classifier also matches, so its membership is
/// visible all the way out in the contract's evidence list. Dropping it would silently downgrade
/// every analyst run from a findings-and-risks attestation to the default lightweight one.
#[test]
fn analyst_is_load_bearing_for_the_level_branch_not_only_for_the_reason() {
    let analyst = infer("analyst", NEUTRAL_TASK);
    // The default fallthrough (`acceptance.ts:118-124`) is what `analyst` would get without the
    // alternation — asserted here so the comparison is against a real, reachable alternative rather
    // than an imagined one.
    let outsider = infer("helper", NEUTRAL_TASK);
    assert_eq!(
        outsider.inferred_reason,
        vec!["default lightweight attestation".to_string()],
        "the control must actually reach the fallthrough branch, otherwise this proves nothing"
    );
    assert_eq!(
        outsider.evidence,
        vec![
            AcceptanceEvidenceKind::ManualNotes,
            AcceptanceEvidenceKind::ResidualRisks,
        ]
    );
    assert_ne!(
        analyst.evidence, outsider.evidence,
        "`analyst`'s alternation membership must change the evidence the contract demands"
    );
    assert_ne!(analyst.criteria, outsider.criteria);
}

/// The other half of upstream `83b9872`: `context-builder` was REMOVED from the alternation in the
/// same edit that added `oracle`. It is now an ordinary name and must fall through to the default
/// lightweight attestation on neutral wording — re-adding it would be as invisible as deleting
/// `oracle` was.
#[test]
fn context_builder_is_no_longer_in_the_alternation() {
    let resolved = infer("context-builder", NEUTRAL_TASK);
    assert_eq!(
        resolved.inferred_reason,
        vec!["default lightweight attestation".to_string()],
        "`context-builder` was dropped from the read-only-agent alternation upstream"
    );
    assert_eq!(
        resolved.evidence,
        vec![
            AcceptanceEvidenceKind::ManualNotes,
            AcceptanceEvidenceKind::ResidualRisks,
        ]
    );
}

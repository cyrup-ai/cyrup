//! R-SA-023 contract injection: appending the machine-parseable `## Acceptance Contract`
//! block to the task text a child receives.

use super::contract::AcceptanceContract;

/// The exact heading this module injects and later scans for — kept as a named constant so
/// [`crate::exec::acceptance::model::format_acceptance_prompt`] (which emits it) and any future re-detection logic never
/// drift out of sync with each other over a hand-typed literal.
pub(crate) const ACCEPTANCE_CONTRACT_HEADING: &str = "## Acceptance Contract";

/// R-SA-023 (MUST) — append a machine-parseable `## Acceptance Contract` block to `task`,
/// instructing the child to emit a fenced `acceptance-report` JSON block as the final part of its
/// answer. MUST run before [`crate::spawn::SpawnedChild::spawn`] (this function performs no I/O
/// and spawns nothing itself — it is pure text augmentation, matching R-SA-024's own "steer at
/// generation time, not after the fact" principle, applied here to the acceptance contract rather
/// than the output-path override `exec/output.rs::inject_output_path_system_prompt` already
/// handles for that separate concern).
///
/// A `contract.is_no_op()` contract (func-SA: "no acceptance contract applies") appends nothing
/// and returns `task` unchanged — there is nothing to instruct the child to report if the
/// orchestrator itself will never gate on it.
///
/// The injected block IS pi's `formatAcceptancePrompt` output (`acceptance.ts:403-457` @v0.43.0),
/// produced by [`crate::exec::acceptance::model::format_acceptance_prompt`] over a [`crate::exec::acceptance::model::ResolvedAcceptanceConfig`]
/// projected from `contract` by [`AcceptanceContract::to_resolved_config`]: the acceptance level,
/// the `- <id>: <must>` criteria list, the `Required evidence: <kinds>` line, the parent's
/// `verify[]` commands, the review gate, the stop rules, and the exact `acceptance-report` JSON
/// template. That template names `criteriaSatisfied` plus every one of
/// `exec/output.rs::ACCEPTANCE_REPORT_COMPANION_KEYS`, so a compliant child's block is reliably
/// picked up by both R-SA-029's final-output extraction AND this module's own
/// `extract_acceptance_report`.
///
/// Before SUBA-C13 this function wrote its own fixed block that mentioned only the required level,
/// the report keys and the `verify[]` list — a child was never told the criteria it was about to be
/// gated on, which meant it had no reason to report them and
/// [`crate::exec::acceptance::model::check_criteria_satisfied`] would have failed every one of them.
///
/// **[CYRUP-DELTA]** upstream joins with a single `\n` (`${task}\n${acceptancePrompt}`,
/// `execution.ts:1038`) and relies on the prompt's own leading empty line for the blank separator;
/// this keeps the pre-existing `\n\n` join and drops that leading line instead, which produces the
/// identical bytes for a non-empty task and, for an EMPTY task, a block with no leading blank line
/// rather than upstream's `"\n## Acceptance Contract…"`.
#[must_use]
pub fn inject_acceptance_contract(task: &str, contract: &AcceptanceContract) -> String {
    if contract.is_no_op() {
        return task.to_string();
    }

    // `formatAcceptancePrompt(effectiveAcceptance, { reportOptional: isAgentContractV1(...) })`
    // (`runs/foreground/execution.ts:1410`). `report_optional` is `false` here because
    // `isAgentContractV1` is `contract?.version === 1` (`shared/agent-contract.ts:3-5`) and this
    // crate has no agent-contract concept at all yet — `runs/shared/agent-contract.ts` is unported
    // — so no cyrup agent can declare one and the predicate is `false` for every run. It is a
    // parameter rather than a hardcoded `false` inside the prompt builder so that porting
    // `agent-contract.ts` is a change at THIS seam only.
    let block = crate::exec::acceptance::model::format_acceptance_prompt(
        &contract.to_resolved_config(),
        false,
    );
    // `format_acceptance_prompt`'s first line is deliberately empty (upstream's `lines[0] = ""`);
    // strip it, since the join below supplies the separator.
    let block = block.strip_prefix('\n').unwrap_or(&block);
    if block.is_empty() {
        return task.to_string();
    }

    if task.is_empty() {
        block.to_string()
    } else {
        format!("{task}\n\n{block}")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::lattice::AcceptanceStatus;
    use crate::exec::acceptance::lattice::contract::ReviewerResult;
    use crate::exec::acceptance::lattice::testsupport::vc;

    // ---------------------------------------------------------------------------------------
    // inject_acceptance_contract (R-SA-023)
    // ---------------------------------------------------------------------------------------

    /// A contract that genuinely disables the gate (pi's `{ level: "none", reason: … }` /
    /// `false` shorthand, the only shapes `explicitAcceptanceCanDisable` accepts,
    /// `acceptance.ts:167-174`) still appends nothing. The INFERRED contract no longer reaches
    /// this state — `inferLevel` has no `none` branch — so the fixture is now an explicit one.
    #[test]
    fn a_gate_disabling_contract_leaves_task_text_unchanged() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        assert!(contract.is_no_op());
        let out = inject_acceptance_contract("Investigate the bug", &contract);
        assert_eq!(out, "Investigate the bug");
    }

    /// The converse, and the actual regression this pairs with: a research/read-only child DOES
    /// get pi's `## Acceptance Contract` block, naming the criterion it will be judged on and the
    /// evidence its `acceptance-report` must carry (`formatAcceptancePrompt`,
    /// `acceptance.ts:403-457`, appended at `execution.ts:1037-1038`).
    #[test]
    fn a_research_child_still_receives_the_acceptance_contract_block() {
        let contract = AcceptanceContract::heuristic_default("researcher", "Investigate the bug");
        let out = inject_acceptance_contract("Investigate the bug", &contract);
        assert!(out.starts_with("Investigate the bug"));
        assert!(out.contains(ACCEPTANCE_CONTRACT_HEADING));
        assert!(out.contains("acceptance-report"));
        assert!(out.contains("Return concrete findings with file paths and severity"));
        assert!(out.contains("review-findings"));
    }

    #[test]
    fn required_contract_appends_a_machine_parseable_acceptance_contract_block() {
        let contract =
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("cargo test")]);
        let out = inject_acceptance_contract("Fix the bug", &contract);
        assert!(out.starts_with("Fix the bug"));
        assert!(out.contains(ACCEPTANCE_CONTRACT_HEADING));
        assert!(out.contains("acceptance-report"));
        assert!(out.contains("criteriaSatisfied"));
        assert!(out.contains("cargo test"));
        assert!(out.contains("verified"));
    }

    #[test]
    fn contract_block_appended_to_empty_task_has_no_leading_blank_lines() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        let out = inject_acceptance_contract("", &contract);
        assert!(out.starts_with(ACCEPTANCE_CONTRACT_HEADING));
    }

    /// G78 — [`AcceptanceContract::to_resolved_config`]'s `Reviewed | Rejected -> Checked` arm,
    /// asserted through its LIVE consumer [`inject_acceptance_contract`] (the other one is
    /// `spawn::chain_graph`'s dynamic-group gate).
    ///
    /// v0.43.0 deleted the `reviewed` LEVEL (`shared/types.ts:639`), so a lattice contract carrying a
    /// [`ReviewerResult`] — the only way `required_level` reaches
    /// [`AcceptanceStatus::Reviewed`] — has no level of its own to project onto. Mapping it to
    /// `Verified` instead would tell the child it is being gated on runtime verification and, worse,
    /// would make `crate::exec::acceptance::model::evaluate_acceptance` reject the run outright for declaring no `verify[]`
    /// commands (`acceptance.ts:1281-1286`). `Checked` is upstream's own spelling of "an
    /// independent reviewer must sign this off": `level: "checked"` plus a `review` gate.
    #[test]
    fn a_reviewer_gated_contract_projects_onto_checked_never_onto_verified() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
            .with_reviewer_result(ReviewerResult {
                approved: true,
                detail: None,
            });
        assert_eq!(
            contract.required_level,
            AcceptanceStatus::Reviewed,
            "premise: this is the Reviewed arm"
        );

        let resolved = contract.to_resolved_config();
        assert_eq!(
            resolved.level,
            crate::exec::acceptance::model::AcceptanceLevel::Checked
        );
        assert!(
            resolved.verify.is_empty(),
            "and it declares no verify[] commands, which is exactly why `Verified` would be wrong"
        );

        let out = inject_acceptance_contract("Land the change", &contract);
        assert!(
            out.contains("Acceptance level: checked"),
            "the child-facing prompt must name `checked`: {out}"
        );
        assert!(
            !out.contains("Acceptance level: verified"),
            "projecting onto `verified` would demand runtime verification this contract never \
             declared: {out}"
        );

        // The sink variant takes the same arm. `AcceptanceContract::explicit` clamps `Rejected`
        // away, so this shape is only reachable by constructing the struct directly — the arm is a
        // defence, and it must not silently become the highest level in the lattice.
        let rejected = AcceptanceContract {
            required_level: AcceptanceStatus::Rejected,
            ..AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
        };
        assert_eq!(
            rejected.to_resolved_config().level,
            crate::exec::acceptance::model::AcceptanceLevel::Checked
        );
    }
}

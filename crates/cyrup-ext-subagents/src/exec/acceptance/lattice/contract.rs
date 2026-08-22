//! [`AcceptanceContract`]: the effective contract for a task — explicit or heuristically
//! inferred — and the reviewer result a `reviewed` verdict consumes.

use super::AcceptanceStatus;

// ============================================================================================
// R-SA-023: Acceptance contract injection
// ============================================================================================

/// One declared verification command (func-SA `acceptance.verify[]`): a literal shell command
/// string plus the per-command execution knobs upstream's `AcceptanceVerifyCommand` carries,
/// executed via a real subprocess (R-SA-032, [`crate::exec::acceptance::lattice::verify::run_verify_commands`]) — never parsed or
/// interpreted beyond being handed to a shell, and never treated as satisfied by anything the
/// child itself claims about it.
///
/// This is deliberately an alias for [`crate::exec::acceptance::model::AcceptanceVerifyCommand`] — the faithful port of
/// upstream `AcceptanceVerifyCommand` (`pi-subagents/src/runs/shared/types.ts` @v0.43.0, whose
/// accepted key set upstream pins as `ACCEPTANCE_VERIFY_KEYS = {id, command, timeoutMs, cwd, env,
/// allowFailure}`, `acceptance.ts:52`) — rather than a second, parallel struct. There is likewise
/// now only ONE runner consuming it ([`crate::exec::acceptance::model::run_verify_command`], via
/// [`crate::exec::acceptance::model::run_memoized_verify_command`]), so a declared command has exactly one meaning. Before SUBA-C12b this alias was a bare `String`, so
/// [`crate::exec::acceptance::lattice::lowering::lower_acceptance_input`] validated all six keys (`crate::exec::acceptance::model::validate_verify_input`) and then
/// discarded five of them: `cwd`/`env`/`timeoutMs` never reached the subprocess and `allowFailure`
/// never reached the gate.
pub type VerifyCommand = crate::exec::acceptance::model::AcceptanceVerifyCommand;

/// The effective acceptance contract for one task (func-SA R-SA-023), resolved BEFORE launch from
/// either an explicit caller-supplied `acceptance` param or heuristic inference from agent
/// name/task text.
///
/// `required_level` names the MINIMUM [`AcceptanceStatus`] the run must reach to be considered
/// accepted; it is meaningless for it to be [`AcceptanceStatus::Rejected`] (rejection is an
/// outcome, never a declared requirement) — callers constructing this by hand should never set
/// it there ([`AcceptanceContract::heuristic_default`]/[`AcceptanceContract::explicit`] both
/// enforce this at construction).
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptanceContract {
    /// The minimum status this run must reach.
    pub required_level: AcceptanceStatus,
    /// Declared verification commands (populated only when `required_level >= Verified`;
    /// meaningless otherwise, but not itself validated to be empty in the lower-level case —
    /// a caller MAY declare `verify[]` commands even at `Checked` as forward-looking metadata;
    /// [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] simply never executes them unless the required level demands it).
    pub verify: Vec<VerifyCommand>,
    /// Whether this contract was explicitly supplied by the caller (an explicit `acceptance`
    /// param) as opposed to heuristically inferred (R-SA-033: "AND the acceptance contract was
    /// explicit AND..." — the post-hoc exit-code correction is gated on this flag specifically,
    /// so a heuristically-inferred contract's rejection is recorded on the ledger but does NOT
    /// by itself flip an otherwise-successful exit code to nonzero).
    pub explicit: bool,
    /// An externally-supplied reviewer result, when present — the sole path by which
    /// [`AcceptanceStatus::Reviewed`] can ever be reached (func-SA §9 open question 10: this
    /// module has no reviewer-dispatch mechanism of its own; it only consumes one a caller
    /// already obtained).
    pub reviewer_result: Option<ReviewerResult>,
    /// Whether this contract may turn the gate OFF outright, or is merely a FLOOR that the
    /// heuristically-inferred level can still raise (pi `explicitAcceptanceCanDisable`,
    /// `runs/shared/acceptance.ts:167-174` @v0.43.0: `explicit.level === "none" && typeof
    /// explicit.reason === "string" && explicit.reason.trim().length > 0`).
    ///
    /// Upstream, only a `level: "none"` that ALSO carries a non-blank `reason` — or the `false`
    /// shorthand, which `normalizeAcceptanceInput` (`:127-132`) rewrites to exactly that shape
    /// with the reason `"disabled by deprecated false shorthand"` — actually disables the gate.
    /// A bare `"none"` string carries no reason at all, so upstream falls through to
    /// `LEVEL_RANK["none"] >= LEVEL_RANK[inferred.level] ? "none" : inferred.level` (`:277-281`)
    /// and, since `none` ranks lowest, always ends up back at the inferred level. Before this
    /// flag existed, [`crate::exec::acceptance::lattice::lowering::lower_acceptance_input`] mapped a bare `"none"` onto an explicit
    /// `NotRequired` contract whose [`is_no_op`](Self::is_no_op) short-circuited the whole gate,
    /// so a one-word policy silently disarmed acceptance that pi still enforces.
    ///
    /// Only ever consulted by [`AcceptanceContract::resolve_effective`]; a contract this crate
    /// builds directly in Rust (rather than lowering from a wire policy) keeps the historical
    /// "an explicit `NotRequired` means the caller wants no gate" reading — see
    /// [`AcceptanceContract::explicit`].
    pub disables_gate: bool,
    /// The resolved acceptance criteria this run is gated on (pi `ResolvedAcceptanceConfig.criteria`,
    /// `shared/types.ts:694-704` @v0.43.0), already through [`crate::exec::acceptance::model::normalize_criteria`] so every entry
    /// carries an `id`, a non-blank `must` and a [`crate::exec::acceptance::model::GateSeverity`].
    ///
    /// [`crate::exec::acceptance::lattice::inject::inject_acceptance_contract`] renders them as pi's `- <id>: <must>` list so the child KNOWS
    /// what it must report, and [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`]'s `Checked` rung enforces them via
    /// [`crate::exec::acceptance::model::check_criteria_satisfied`] — pi `evaluateAcceptance`, `acceptance.ts:1297-1321`. Empty
    /// means "nothing declared", which is enforcement-neutral: pi's own `formatAcceptancePrompt`
    /// falls back to `- Return the requested result.` and `checkCriteriaSatisfied` over an empty
    /// list yields no checks.
    pub criteria: Vec<crate::exec::acceptance::model::ResolvedAcceptanceGate>,
    /// The evidence kinds the child's own `acceptance-report` block MUST carry (pi
    /// `ResolvedAcceptanceConfig.evidence`). Rendered into the prompt as pi's
    /// `Required evidence: <kinds>` line and enforced by [`crate::exec::acceptance::model::run_structural_checks`] —
    /// including the REAL `git status --short` subprocess pi runs for
    /// [`crate::exec::acceptance::model::AcceptanceEvidenceKind::NoStagedFiles`] (`checkNoStagedFiles`,
    /// `acceptance.ts:939-948`).
    pub evidence: Vec<crate::exec::acceptance::model::AcceptanceEvidenceKind>,
    /// The declared review gate (pi `ResolvedAcceptanceConfig.review`). Rendered into the prompt as
    /// pi's `Review gate: required|optional[ by <agent>]` / `Review focus:` lines, and consulted at
    /// [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`]'s `Reviewed` rung: a gate declaring `required: false` is pi's
    /// `optionalReview` (`acceptance.ts:1321-1335`), which does NOT reject a run that reached
    /// `Reviewed` with no [`ReviewerResult`] in hand.
    pub review: Option<crate::exec::acceptance::model::ReviewSetting>,
    /// The declared stop rules (pi `ResolvedAcceptanceConfig.stopRules`). Prompt-only, exactly as
    /// upstream: `formatAcceptancePrompt` emits them (`acceptance.ts:432-434`) and no upstream
    /// runtime check ever reads them back.
    pub stop_rules: Vec<String>,
}

impl AcceptanceContract {
    /// Build an explicit, caller-supplied contract (the "explicit `acceptance` param" half of
    /// R-SA-023's resolution rule). `required_level` MUST NOT be [`AcceptanceStatus::Rejected`] —
    /// callers passing that are almost certainly confusing an outcome with a requirement;
    /// this constructor clamps it down to [`AcceptanceStatus::Reviewed`] (the highest real,
    /// requestable level) defensively rather than accepting a nonsensical contract, since this
    /// crate's no-panic policy forbids failing loudly here and a silently-dropped contract would
    /// be worse than a clamped one.
    ///
    /// A contract built here with [`AcceptanceStatus::NotRequired`] DISABLES the gate outright
    /// (`disables_gate`), i.e. it is upstream's `{ level: "none", reason: <non-blank> }` rather
    /// than its reasonless bare `"none"`: an in-process caller that hands `run_sync` a
    /// `NotRequired` contract in Rust has stated its intent directly and there is no reason field
    /// for it to have omitted. Wire policies that must NOT be able to disable the gate are lowered
    /// through [`AcceptanceContract::explicit_floor`] instead.
    #[must_use]
    pub fn explicit(required_level: AcceptanceStatus, verify: Vec<VerifyCommand>) -> Self {
        let required_level = clamp_requestable_level(required_level);
        Self {
            required_level,
            verify,
            explicit: true,
            reviewer_result: None,
            disables_gate: required_level == AcceptanceStatus::NotRequired,
            criteria: Vec::new(),
            evidence: Vec::new(),
            review: None,
            stop_rules: Vec::new(),
        }
    }

    /// Attach the resolved `criteria`/`evidence`/`review`/`stopRules` half of an authored
    /// `acceptance` policy (builder-style), the four fields [`crate::exec::acceptance::lattice::lowering::lower_acceptance_input`] used to
    /// validate and then DISCARD.
    ///
    /// Splitting this off [`AcceptanceContract::explicit`] keeps that constructor's existing
    /// signature (and its ~40 in-tree call sites) intact: a contract built in Rust with no declared
    /// policy is exactly the pre-existing shape, and only a wire-lowered policy carries the extra
    /// four. Criteria are expected to have already been through
    /// [`crate::exec::acceptance::model::normalize_criteria`] — this setter does no normalization of its own so the single
    /// normalization rule stays in the [`crate::exec::acceptance::model`] port that owns it.
    #[must_use]
    pub fn with_policy(
        mut self,
        criteria: Vec<crate::exec::acceptance::model::ResolvedAcceptanceGate>,
        evidence: Vec<crate::exec::acceptance::model::AcceptanceEvidenceKind>,
        review: Option<crate::exec::acceptance::model::ReviewSetting>,
        stop_rules: Vec<String>,
    ) -> Self {
        self.criteria = criteria;
        self.evidence = evidence;
        self.review = review;
        self.stop_rules = stop_rules;
        self
    }

    /// Build an explicit, caller-supplied contract that acts only as a **floor**: the
    /// heuristically-inferred level may still raise it, and it can never disable the gate
    /// (pi `explicitAcceptanceCanDisable` returning `false`, `acceptance.ts:167-174` @v0.43.0).
    ///
    /// This is the shape every wire-lowered policy takes EXCEPT the two upstream treats as a
    /// genuine "off" switch (`acceptance: false`, and `{ level: "none", reason: "…" }` with a
    /// non-blank reason). For any level above `none` the distinction is invisible — a non-
    /// `NotRequired` contract has nothing to disable — so this constructor differs from
    /// [`AcceptanceContract::explicit`] only for `NotRequired`.
    #[must_use]
    pub fn explicit_floor(required_level: AcceptanceStatus, verify: Vec<VerifyCommand>) -> Self {
        Self {
            disables_gate: false,
            ..Self::explicit(required_level, verify)
        }
    }

    /// Attach an externally-supplied reviewer result to this contract (builder-style), raising
    /// `required_level` to at least [`AcceptanceStatus::Reviewed`] if it was not already — a
    /// contract that HAS a reviewer result but a lower declared level would otherwise never
    /// actually consult it.
    #[must_use]
    pub fn with_reviewer_result(mut self, result: ReviewerResult) -> Self {
        self.reviewer_result = Some(result);
        if self.required_level < AcceptanceStatus::Reviewed {
            self.required_level = AcceptanceStatus::Reviewed;
        }
        // A contract carrying a reviewer result requires `Reviewed` by construction, so it is no
        // longer the `level: "none"` shape `disables_gate` describes.
        self.disables_gate = false;
        self
    }

    /// Build the heuristically-inferred default contract (the "heuristic inference from agent
    /// name/task text" half of R-SA-023's resolution rule), used only when the caller supplied no
    /// explicit `acceptance` param at all.
    ///
    /// This IS pi's `inferLevel` (`runs/shared/acceptance.ts:69-125` @v0.34.0), reached through the
    /// [`crate::exec::acceptance::model`] port's faithful line-for-line copy of it, so the four-way decision tree, the
    /// criteria strings and the per-branch evidence sets are upstream's rather than a second,
    /// drifting approximation:
    ///
    /// | branch | level | evidence |
    /// |---|---|---|
    /// | risky / dynamic / async write (`:88-96`) | `reviewed` | the full `reviewed` set + a required `reviewer` gate |
    /// | write-capable worker or task (`:98-105`) | `checked` | the full `checked` set |
    /// | read-only agent or read-only task wording (`:107-116`) | `attested` | `review-findings`, `residual-risks` |
    /// | everything else (`:118-124`) | `attested` | `manual-notes`, `residual-risks` |
    ///
    /// **`inferLevel` has no `"none"` branch**, which is the property that matters here: upstream
    /// appends a `## Acceptance Contract` block to essentially every child task
    /// (`formatAcceptancePrompt` returns `""` only for `level === "none"`, `acceptance.ts:408`, and
    /// `execution.ts:1037-1038` appends it unconditionally) and always produces a real ledger.
    /// Before this change this function ran the enum-lattice
    /// [`crate::exec::completion_guard::expects_implementation_mutation`] classifier instead and
    /// returned [`AcceptanceStatus::NotRequired`] for anything that did not read as
    /// implementation-expecting — so a reviewer/scout/researcher/summariser child was sent a
    /// materially different prompt from pi's (no criteria, no required evidence, no
    /// `acceptance-report` instruction at all: [`crate::exec::acceptance::lattice::inject::inject_acceptance_contract`] returns the task
    /// verbatim for an [`is_no_op`](Self::is_no_op) contract) and its result carried
    /// `acceptance: not-required` where pi reports `attested` or `rejected`.
    ///
    /// A heuristically-inferred contract is NOT explicit ([`Self::explicit`] stays `false`), so its
    /// rejection is recorded on the ledger and never flips an otherwise-clean exit code — pi gates
    /// that correction on `result.acceptance.explicit` too (`execution.ts:1229`), which is what
    /// makes always-inferring safe rather than a mass run-failure.
    ///
    /// **[CYRUP-DELTA]** the `async`/`dynamic`/`dynamicGroup`/`mode` inputs to `inferLevel` are
    /// left at their defaults: this signature classifies on agent name + task text alone, which is
    /// all its callers have. A dynamic step's own group gate is resolved separately, through
    /// [`crate::exec::acceptance::lattice::lowering::lower_acceptance_input`] on the declared policy (`spawn/chain_graph.rs`), so the
    /// `dynamic fanout context` escalation is reachable there rather than lost outright.
    #[must_use]
    pub fn heuristic_default(agent_local_name: &str, task: &str) -> Self {
        let inferred = crate::exec::acceptance::model::resolve_effective_acceptance(&crate::exec::acceptance::model::AcceptanceResolveInput {
            explicit: None,
            agent_name: agent_local_name.to_string(),
            task: Some(task.to_string()),
            mode: None,
            is_async: false,
            dynamic: false,
            dynamic_group: false,
        });
        Self {
            // `resolve_effective_acceptance` with no explicit input returns `inferred.level`
            // verbatim, and `infer_level` only ever yields `attested`/`checked` (v0.43.0 removed
            // the `reviewed` level; its risky branch now returns `checked` plus a required review
            // gate, `acceptance.ts:114-120`) — the `Auto`/`None`/`Verified` arms below are
            // unreachable and map to the nearest real level rather than reintroducing a silent
            // no-op contract.
            required_level: match inferred.level {
                crate::exec::acceptance::model::AcceptanceLevel::Verified => AcceptanceStatus::Verified,
                crate::exec::acceptance::model::AcceptanceLevel::Checked => AcceptanceStatus::Checked,
                crate::exec::acceptance::model::AcceptanceLevel::Attested
                | crate::exec::acceptance::model::AcceptanceLevel::Auto
                | crate::exec::acceptance::model::AcceptanceLevel::None => AcceptanceStatus::Attested,
            },
            // Nothing was explicitly declared to run: `inferLevel` never produces `verify[]`
            // commands (only an authored policy can), and `stopRules` is likewise explicit-only.
            verify: Vec::new(),
            explicit: false,
            reviewer_result: None,
            disables_gate: false,
            criteria: inferred.criteria,
            evidence: inferred.evidence,
            review: inferred.review,
            stop_rules: Vec::new(),
        }
    }

    /// Combine an explicit, caller-supplied contract (if any) with the heuristically-inferred one
    /// — R-SA-023's resolution rule, and a direct port of pi `resolveEffectiveAcceptance`'s level
    /// arithmetic (`runs/shared/acceptance.ts:344-401` @v0.43.0):
    ///
    /// ```text
    /// level = explicitAcceptanceCanDisable(explicit) ? "none"
    ///       : explicitLevel === "auto"               ? inferred.level
    ///       : LEVEL_RANK[explicitLevel] >= LEVEL_RANK[inferred.level] ? explicitLevel
    ///                                                                 : inferred.level
    /// ```
    ///
    /// The load-bearing property is the MAX in that last branch: an explicit level may only ever
    /// RAISE the inferred floor, never lower it. Before this function existed, `run_sync` did
    /// `opts.acceptance.clone().unwrap_or_else(|| heuristic_default(...))` — explicit and inferred
    /// were mutually exclusive, so `acceptance: "attested"` on a write-capable task was honoured
    /// verbatim where pi escalates it to the inferred `checked`, and a bare `acceptance: "none"`
    /// (which cannot disable upstream, see [`disables_gate`](Self::disables_gate)) turned the gate
    /// off entirely.
    ///
    /// `explicit == None` is pi's `explicitLevel === "auto"` — [`crate::exec::acceptance::lattice::lowering::lower_acceptance_input`] maps
    /// both an absent `acceptance` param and a literal `"auto"` onto `None` — and yields the
    /// inferred contract unchanged.
    ///
    /// **[CYRUP-DELTA]** upstream also feeds `async`/`dynamic`/`dynamicGroup` into `inferLevel`
    /// and downgrades `review.required` when a `reviewed` level was inferred rather than asked for
    /// (`acceptance.ts:286-289`); neither has an input here, because
    /// [`AcceptanceContract::heuristic_default`] classifies on agent name + task text alone. That
    /// still leaves the `reviewed` rung reachable — `inferLevel`'s risky branch also fires on task
    /// WORDING (`release`/`migration`/`security`/…, `acceptance.ts:109`), which this signature does
    /// see — so what is missing is only the three boolean escalations and the review downgrade,
    /// both tracked separately from this function's own concern, which is strictly the combination
    /// rule.
    ///
    /// This function combines LEVELS only. The inferred contract's `criteria`/`evidence`/`review`
    /// are dropped whenever an explicit contract is present, where upstream merges them
    /// (`acceptance.ts:283-292`); an explicit policy that declares none of the three therefore
    /// gates on nothing rather than on `requiredEvidenceForLevel(level)`.
    #[must_use]
    pub fn resolve_effective(
        explicit: Option<Self>,
        agent_local_name: &str,
        task: &str,
    ) -> Self {
        let inferred = Self::heuristic_default(agent_local_name, task);
        let Some(mut contract) = explicit else {
            return inferred;
        };
        if contract.disables_gate {
            return contract;
        }
        // MAX(explicit, inferred) by lattice rank. `AcceptanceStatus`'s derived `Ord` IS that rank
        // (the enum's own doc comment: declaration order is normative), and neither side can be
        // `Rejected` — `heuristic_default` never produces it and `explicit`/`explicit_floor` clamp
        // it away — so the sink variant cannot leak in through this comparison.
        if inferred.required_level > contract.required_level {
            contract.required_level = inferred.required_level;
        }
        contract
    }

    /// Whether this contract requires no gate evaluation at all — `required_level ==
    /// NotRequired` and no reviewer result is attached. [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] short-circuits on
    /// this so a task with no acceptance requirement never even inspects the transcript.
    #[must_use]
    pub fn is_no_op(&self) -> bool {
        self.required_level == AcceptanceStatus::NotRequired && self.reviewer_result.is_none()
    }

    /// Project this contract onto the [`crate::exec::acceptance::model`] port's [`crate::exec::acceptance::model::ResolvedAcceptanceConfig`], the
    /// shape pi's own `formatAcceptancePrompt` consumes (`acceptance.ts:407-457`) — so the child-facing
    /// prompt is rendered by the faithful port rather than by a second, drifting copy of it.
    ///
    /// The level mapping is total both ways for every level a policy can actually declare
    /// ([`crate::exec::acceptance::lattice::lowering::lower_acceptance_input`] only ever produces `NotRequired`/`Attested`/`Checked`/`Verified`/
    /// `Reviewed`). The two [`AcceptanceStatus`] variants with no [`crate::exec::acceptance::model::AcceptanceLevel`]
    /// counterpart are outcomes rather than requirements and are unreachable as a `required_level`:
    /// [`AcceptanceStatus::Claimed`] is the self-report floor [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] computes, and
    /// [`AcceptanceStatus::Rejected`] is clamped away by [`AcceptanceContract::explicit`]. Both are
    /// mapped to their nearest declarable neighbour rather than silently dropping the whole prompt.
    ///
    /// `inferred_reason` is empty because [`AcceptanceContract`] carries no reasons field of its
    /// own — [`AcceptanceContract::heuristic_default`] does now produce upstream's reason strings
    /// (via [`crate::exec::acceptance::model::resolve_effective_acceptance`]) but drops them at this seam. The field is
    /// prompt-irrelevant (`formatAcceptancePrompt` never reads it); it surfaces upstream only on
    /// the LEDGER's `inferredReason`, which this crate's narrower [`crate::exec::acceptance::lattice::AcceptanceLedger`] does not
    /// have either.
    #[must_use]
    pub fn to_resolved_config(&self) -> crate::exec::acceptance::model::ResolvedAcceptanceConfig {
        let level = match self.required_level {
            AcceptanceStatus::NotRequired => crate::exec::acceptance::model::AcceptanceLevel::None,
            AcceptanceStatus::Claimed | AcceptanceStatus::Attested => {
                crate::exec::acceptance::model::AcceptanceLevel::Attested
            }
            AcceptanceStatus::Checked => crate::exec::acceptance::model::AcceptanceLevel::Checked,
            AcceptanceStatus::Verified => crate::exec::acceptance::model::AcceptanceLevel::Verified,
            // v0.43.0 deleted the `reviewed` LEVEL (`shared/types.ts:639`), so a lattice contract that
            // requires an independent reviewer has no level of its own to project onto. Upstream
            // expresses exactly that shape as `level: "checked"` plus `review.required` — which
            // this contract already carries separately in `review` — so `Reviewed` projects to
            // `Checked`, NOT to `Verified` (which would additionally demand `verify[]` commands
            // this contract never declared and reject the run for their absence,
            // `acceptance.ts:1281-1286`).
            AcceptanceStatus::Reviewed | AcceptanceStatus::Rejected => {
                crate::exec::acceptance::model::AcceptanceLevel::Checked
            }
        };
        crate::exec::acceptance::model::ResolvedAcceptanceConfig {
            level,
            explicit: self.explicit,
            inferred_reason: Vec::new(),
            criteria: self.criteria.clone(),
            evidence: self.evidence.clone(),
            verify: self.verify.clone(),
            review: self.review.clone(),
            stop_rules: self.stop_rules.clone(),
            reason: None,
        }
    }
}

/// Defensive clamp for [`AcceptanceContract::explicit`]'s `required_level` — see that
/// constructor's doc comment.
fn clamp_requestable_level(level: AcceptanceStatus) -> AcceptanceStatus {
    match level {
        // The highest level a POLICY can request at v0.43.0 is `verified` — `reviewed` is an
        // achieved status only (`shared/types.ts:639`, `acceptance.ts:54`), reachable from a real
        // [`ReviewerResult`] via [`AcceptanceContract::with_reviewer_result`] and never by asking.
        AcceptanceStatus::Rejected => AcceptanceStatus::Verified,
        other => other,
    }
}

/// An externally-supplied, genuine independent reviewer result (func-SA R-SA-032's `reviewed`
/// level; DI-SA-5's "genuine independent reviewer result, not a worker's own claim of having been
/// reviewed"). Deliberately opaque about HOW it was produced — the dispatch mechanism is out of
/// scope for this module (func-SA §9 open question 10) — this type only carries the outcome a
/// caller already obtained through some other, unspecified path.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReviewerResult {
    /// Whether the independent reviewer approved the run.
    pub approved: bool,
    /// The reviewer's own detail/rationale text, when available.
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::lattice::testsupport::passed;
    use crate::exec::acceptance::lattice::testsupport::vc;
    use crate::exec::acceptance::lattice::testsupport::vc_timeout;


    use std::time::Duration;




    /// The ONE verify runner (`crate::exec::acceptance::model::run_verify_command`, upstream `runVerifyCommand`) under the
    /// short local name the single-command tests below used when this module carried a second copy
    /// of it. There is no second copy any more; only the alias survives.
    use crate::exec::acceptance::model::run_verify_command as run_one_verify_command;


    /// The combined capture the retired `VerifyCommandResult.output_tail` held, reassembled from
    /// upstream's separate `stdout`/`stderr` in the same order the old field concatenated them.
    fn output_tail(result: &crate::exec::acceptance::model::AcceptanceVerifyResult) -> String {
        format!(
            "{}{}",
            result.stdout.as_deref().unwrap_or_default(),
            result.stderr.as_deref().unwrap_or_default()
        )
    }


    // ---------------------------------------------------------------------------------------
    // AcceptanceContract construction
    // ---------------------------------------------------------------------------------------

    #[test]
    fn heuristic_default_requires_checked_for_implementation_expecting_tasks() {
        let contract = AcceptanceContract::heuristic_default("worker", "Implement the fix");
        assert_eq!(contract.required_level, AcceptanceStatus::Checked);
        assert!(!contract.explicit);
        assert!(contract.verify.is_empty());
    }


    /// pi `inferLevel`'s read-only branch (`acceptance.ts:107-116` @v0.34.0): read-only TASK
    /// WORDING infers `attested` with the findings criterion and the review-findings evidence
    /// pair — never `none`, which `inferLevel` has no branch for at all. (This test previously
    /// asserted `NotRequired`/`is_no_op`, the divergence.)
    #[test]
    fn heuristic_default_attests_review_only_tasks_rather_than_disarming() {
        let contract =
            AcceptanceContract::heuristic_default("worker", "Review only: return findings");
        assert_eq!(contract.required_level, AcceptanceStatus::Attested);
        assert!(!contract.is_no_op(), "there is always something to attest");
        assert_eq!(contract.criteria.len(), 1);
        assert_eq!(
            contract.criteria[0].must,
            "Return concrete findings with file paths and severity when applicable"
        );
        assert_eq!(
            contract.evidence,
            vec![
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ReviewFindings,
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ResidualRisks,
            ]
        );
    }


    /// The read-only AGENT branch of the same tree — `reviewer|oracle|scout|researcher|analyst`
    /// (`acceptance.ts:99` @ v0.43.0) — reached by agent name alone, with no read-only wording in
    /// the task.
    /// CROSS-CUTTING (batch 9): G97 made `advisor` an ALIAS of `oracle`, G99 put `oracle` (and not
    /// `advisor`) into the read-only-agent alternation (`acceptance.ts:99` @v0.43.0), and G83 put
    /// `advisor` into `isReviewerStyleAgent` (`task-intent.ts:138-140`). Three groups, one outcome: the
    /// acceptance contract a caller gets must not depend on WHICH spelling of the same agent it
    /// used.
    ///
    /// This is load-bearing rather than cosmetic. `advisor` is absent from the alternation, so it
    /// reaches the same verdict as `oracle` only because two independent mechanisms agree —
    /// name canonicalization at dispatch, and G83's reviewer-style classifier. If either is
    /// narrowed, an `advisor` call silently starts running a STRICTER gate than the identical
    /// `oracle` call, and no other test in the crate compares the two.
    ///
    /// `seer` is the control: an unrelated name genuinely does diverge on a write-shaped task, which
    /// proves this test is comparing something that can differ rather than asserting a constant.
    #[test]
    fn an_alias_infers_the_same_acceptance_contract_as_the_agent_it_names() {
        const TASKS: &[&str] = &[
            "Investigate the bug",
            "Implement the fix",
            "Update the parser and add a test",
            "Say hello",
        ];

        for task in TASKS {
            let via_alias = AcceptanceContract::heuristic_default("advisor", task);
            let via_name = AcceptanceContract::heuristic_default("oracle", task);
            assert_eq!(
                via_alias.required_level, via_name.required_level,
                "`advisor` is an alias of `oracle`; the inferred contract for {task:?} must not \
                 depend on which spelling was used"
            );
        }

        // Control: the invariance above is a real agreement between two agent names, not an
        // artifact of every name inferring the same thing.
        let unrelated =
            AcceptanceContract::heuristic_default("seer", "Update the parser and add a test");
        let oracle =
            AcceptanceContract::heuristic_default("oracle", "Update the parser and add a test");
        assert_ne!(
            unrelated.required_level, oracle.required_level,
            "a write-shaped task must infer a stricter contract for a non-reviewer-style agent, \
             otherwise this test proves nothing"
        );
    }


    #[test]
    fn heuristic_default_attests_a_research_agent_on_neutral_task_wording() {
        let contract = AcceptanceContract::heuristic_default("researcher", "Investigate the bug");
        assert_eq!(contract.required_level, AcceptanceStatus::Attested);
        assert_eq!(
            contract.evidence,
            vec![
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ReviewFindings,
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ResidualRisks,
            ]
        );
    }


    /// `inferLevel`'s final fallthrough (`acceptance.ts:118-124`): an agent and a task that match
    /// no branch at all still attest, with the lightweight manual-notes evidence pair.
    #[test]
    fn heuristic_default_falls_through_to_lightweight_attestation() {
        let contract = AcceptanceContract::heuristic_default("helper", "Say hello");
        assert_eq!(contract.required_level, AcceptanceStatus::Attested);
        assert_eq!(
            contract.criteria[0].must,
            "Return a concise result and residual risks when applicable"
        );
        assert_eq!(
            contract.evidence,
            vec![
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ManualNotes,
                crate::exec::acceptance::model::AcceptanceEvidenceKind::ResidualRisks,
            ]
        );
    }


    #[test]
    fn explicit_contract_is_marked_explicit_and_carries_verify_commands() {
        let contract = AcceptanceContract::explicit(
            AcceptanceStatus::Verified,
            vec![vc("true")],
        );
        assert!(contract.explicit);
        assert_eq!(contract.required_level, AcceptanceStatus::Verified);
        assert_eq!(contract.verify, vec!["true".to_string()]);
    }


    #[test]
    /// The clamp target is `Verified`, not `Reviewed`: v0.43.0 removed `reviewed` from
    /// `AcceptanceLevel` (`shared/types.ts:639`), so the highest level a POLICY can request is `verified`
    /// and `Reviewed` is reachable only from a real [`ReviewerResult`].
    fn explicit_contract_clamps_a_nonsensical_rejected_requested_level() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Rejected, vec![]);
        assert_eq!(contract.required_level, AcceptanceStatus::Verified);
    }


    // ---------------------------------------------------------------------------------------
    // resolve_effective: pi `resolveEffectiveAcceptance`'s combination rule
    // (`runs/shared/acceptance.ts:344-401` @v0.43.0)
    // ---------------------------------------------------------------------------------------

    /// The MAX at `acceptance.ts:277-281` — an explicit level may only ever RAISE the inferred
    /// floor. Pre-fix, `run_sync` used `opts.acceptance.unwrap_or_else(heuristic_default)`, so an
    /// explicit `attested` REPLACED the inferred `checked` on a write-capable task and the run was
    /// gated more weakly than the identical policy is under pi.
    #[test]
    fn an_explicit_level_below_the_inferred_one_is_raised_to_the_inferred_floor() {
        let inferred = AcceptanceContract::heuristic_default("worker", "Implement the fix");
        assert_eq!(inferred.required_level, AcceptanceStatus::Checked, "premise");

        let effective = AcceptanceContract::resolve_effective(
            Some(AcceptanceContract::explicit_floor(
                AcceptanceStatus::Attested,
                vec![],
            )),
            "worker",
            "Implement the fix",
        );

        assert_eq!(
            effective.required_level,
            AcceptanceStatus::Checked,
            "max(attested, checked) is checked — the explicit level is a floor, not a replacement"
        );
        assert!(
            effective.explicit,
            "it is still an explicitly-declared contract, so R-SA-033's correction stays armed"
        );
    }


    /// The other side of the same expression: an explicit level ABOVE the inferred one wins, and
    /// its declared `verify[]` commands survive the combination.
    #[test]
    fn an_explicit_level_above_the_inferred_one_wins_and_keeps_its_verify_commands() {
        let effective = AcceptanceContract::resolve_effective(
            Some(AcceptanceContract::explicit_floor(
                AcceptanceStatus::Verified,
                vec![vc("true")],
            )),
            "researcher",
            "Investigate the bug",
        );
        assert_eq!(effective.required_level, AcceptanceStatus::Verified);
        assert_eq!(effective.verify, vec!["true".to_string()]);
    }


    /// `explicit == None` is pi's `explicitLevel === "auto"` branch: the inferred contract, whole.
    #[test]
    fn no_explicit_contract_yields_the_inferred_one() {
        let effective =
            AcceptanceContract::resolve_effective(None, "worker", "Implement the fix");
        assert_eq!(effective.required_level, AcceptanceStatus::Checked);
        assert!(!effective.explicit, "an inferred contract never arms R-SA-033");
    }


    /// An in-process caller that builds `AcceptanceContract::explicit(NotRequired, …)` directly in
    /// Rust (as several of this crate's own callers and tests do) has stated its intent with no
    /// `reason` field to omit, so that form keeps disabling the gate.
    #[test]
    fn a_rust_constructed_not_required_contract_still_disables_the_gate() {
        let effective = AcceptanceContract::resolve_effective(
            Some(AcceptanceContract::explicit(
                AcceptanceStatus::NotRequired,
                vec![],
            )),
            "worker",
            "Implement the fix",
        );
        assert_eq!(effective.required_level, AcceptanceStatus::NotRequired);
        assert!(effective.is_no_op());
    }


    #[test]
    fn with_reviewer_result_raises_required_level_to_at_least_reviewed() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
            .with_reviewer_result(ReviewerResult {
                approved: true,
                detail: None,
            });
        assert_eq!(contract.required_level, AcceptanceStatus::Reviewed);
    }


    // ---------------------------------------------------------------------------------------
    // run_verify_commands / run_one_verify_command: REAL subprocess execution (no mocks)
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_command_that_exits_zero_is_recorded_as_passed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command(&vc("exit 0"), dir.path()).await;
        assert!(passed(&result));
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::Passed);
        assert_eq!(result.stderr, None, "a clean command reports no diagnostic text");
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_command_that_exits_nonzero_is_recorded_as_failed_with_real_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command(&vc("exit 7"), dir.path()).await;
        assert!(!passed(&result));
        assert_eq!(result.exit_code, Some(7));
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_tail_captures_real_combined_stdout_and_stderr() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command(
            &vc("echo out-marker; echo err-marker 1>&2; exit 1"),
            dir.path(),
        )
        .await;
        assert!(!passed(&result));
        // Upstream keeps the two streams SEPARATE (`stdout`/`stderr`, `acceptance.ts:1194-1195`),
        // so this asserts each marker landed on its own stream — strictly more than the retired
        // combined `output_tail` could say.
        assert!(result.stdout.as_deref().unwrap_or_default().contains("out-marker"));
        assert!(result.stderr.as_deref().unwrap_or_default().contains("err-marker"));
        assert!(output_tail(&result).contains("out-marker"));
        assert!(output_tail(&result).contains("err-marker"));
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn command_runs_in_the_declared_working_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("marker.txt"), "hi").expect("seed file");
        let result =
            run_one_verify_command(&vc("test -f marker.txt"), dir.path()).await;
        assert!(passed(&result), "the file must be visible relative to cwd");
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hanging_command_times_out_and_is_recorded_as_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command(&vc_timeout("sleep 5", Duration::from_millis(100)), dir.path())
            .await;
        assert!(!passed(&result));
        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::TimedOut);
        // `stderr: trimOutput(redactVerifyEnv(stderr || abortMessage || "Acceptance verification
        // timed out.", …))` (`acceptance.ts:1174`) — the command printed nothing, so upstream's
        // literal fallback is what the ledger carries. Was cyrup's own invented
        // "verify command exceeded its 100ms timeout and was terminated".
        assert_eq!(result.stderr.as_deref(), Some(crate::exec::acceptance::model::VERIFY_TIMED_OUT_MESSAGE));
    }


    // ---------------------------------------------------------------------------------------
    // SUBA-027 regression: a timed-out verify[] command must be KILLED, never abandoned.
    //
    // Before the fix, `run_one_verify_command` raced `tokio::time::timeout` against
    // `child.wait_with_output()`, whose `self`-consuming future swallowed the only `Child`
    // handle; the elapsed arm dropped it with no `kill_on_drop`, so a hung `cargo test` survived
    // its own timeout for the machine's uptime — in a process group `process_group(0)` had
    // deliberately detached from the terminal, so Ctrl-C could not reach it either. Every
    // assertion below probes the OS directly with `kill(pid, 0)`, never this crate's bookkeeping.
    // ---------------------------------------------------------------------------------------

    /// Poll `kill(pid, 0)` until it reports ESRCH (the pid no longer exists), up to `timeout`.
    /// Polling rather than probing once because a killed GRANDchild is reparented to init and
    /// stays a zombie for the few microseconds before init reaps it.
    #[cfg(unix)]
    async fn wait_for_pid_gone(pid: i32, timeout: Duration) -> bool {
        let target = nix::unistd::Pid::from_raw(pid);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if nix::sys::signal::kill(target, None).is_err() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }


    /// Poll for `path` to contain a parseable pid, up to `timeout`.
    #[cfg(unix)]
    async fn wait_for_published_pid(path: &std::path::Path, timeout: Duration) -> i32 {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(raw) = std::fs::read_to_string(path)
                && let Ok(pid) = raw.trim().parse::<i32>()
            {
                return pid;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the fixture never published its pid to {} within {timeout:?}",
                path.display()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }


    /// The headline SUBA-027 assertion: after the timeout elapses, the command's own OS process
    /// is gone. `exec` is load-bearing in the fixture — a shell that merely *forks* `sleep` would
    /// let the pid we publish differ from the pid the shell holds, so the test would prove
    /// nothing about which process actually got signalled.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timed_out_verify_command_is_killed_not_abandoned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let result = run_one_verify_command(
            &vc_timeout("echo $$ > pid; exec sleep 300", Duration::from_millis(200)),
            dir.path(),
        )
        .await;

        assert!(!passed(&result));
        let pid = wait_for_published_pid(&pid_file, Duration::from_secs(5)).await;
        assert!(
            wait_for_pid_gone(pid, Duration::from_secs(5)).await,
            "verify command pid {pid} must be gone once run_one_verify_command returns — a \
             timed-out command has to be killed, not abandoned"
        );
    }


    /// SUBA-028 — a CANCELLED run's acceptance verification stops now, not after a full
    /// per-command `timeoutMs` (pi's `options.signal` → `abortVerification`,
    /// `acceptance.ts:1180-1181`).
    ///
    /// THE USER ACTION: the user hits Ctrl-C (or a parent timeout fires) while the acceptance gate
    /// is running `cargo test`. Before this the token reached the CHILD and stopped there — the
    /// verify command kept running to its own deadline, once per remaining command, so "stop"
    /// could take minutes. The assertion is the elapsed wall clock against a 30-SECOND command
    /// budget: a two-order-of-magnitude gap that no amount of scheduling jitter can close, so this
    /// is not a marginal timing test.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_run_aborts_its_verify_command_instead_of_waiting_out_the_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let cancel = cyrup_core::CancelToken::new();
        // 30s budget, cancelled ~150ms in: the command cannot end on its own timeout in the window
        // this test allows, so a `timed-out` verdict here can only have come from the cancel arm.
        let command = vc_timeout("echo $$ > pid; exec sleep 300", Duration::from_secs(30));

        let canceller = {
            let cancel = cancel.clone();
            let pid_file = pid_file.clone();
            tokio::spawn(async move {
                // Cancel only once the command is genuinely running — otherwise this would be the
                // already-aborted branch and would prove nothing about a MID-command abort.
                let pid = wait_for_published_pid(&pid_file, Duration::from_secs(10)).await;
                cancel.cancel();
                pid
            })
        };

        let started = std::time::Instant::now();
        let result =
            crate::exec::acceptance::model::run_verify_command_with_cancel(&command, dir.path(), &cancel).await;
        let elapsed = started.elapsed();
        let pid = canceller.await.expect("canceller task");

        assert!(!passed(&result), "an aborted command never passes: {result:?}");
        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::TimedOut, "{result:?}");
        assert!(
            elapsed < Duration::from_secs(10),
            "the cancel must end the command promptly, not at its own 30s deadline: {elapsed:?}"
        );
        assert!(
            wait_for_pid_gone(pid, Duration::from_secs(5)).await,
            "the aborted command's pid {pid} must be killed, not abandoned — pi's abort path IS \
             its timeout path"
        );
    }


    /// Group targeting is the whole reason `send_signal` negates the pid: the command's own
    /// descendants must die with it. Here the shell stays alive in `wait` while a background
    /// `sleep` (a grandchild of this test process, in the same group thanks to
    /// `process_group(0)`) holds the real work. A pid-only kill would reap the shell and orphan
    /// the sleep into a detached group nothing holds a handle to.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timed_out_verify_command_kills_its_whole_process_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let descendant_pid_file = dir.path().join("descendant");
        let result = run_one_verify_command(
            &vc_timeout("sleep 300 & echo $! > descendant; wait", Duration::from_millis(200)),
            dir.path(),
        )
        .await;

        assert!(!passed(&result));
        let pid = wait_for_published_pid(&descendant_pid_file, Duration::from_secs(5)).await;
        assert!(
            wait_for_pid_gone(pid, Duration::from_secs(5)).await,
            "the descendant pid {pid} the verify command spawned must die with it — the kill \
             targets the command's process GROUP, not just its direct pid"
        );
    }


    /// SUBA-027 regression: a verify command that EXITS PROMPTLY but leaves a backgrounded
    /// descendant holding the inherited stdout/stderr must still return inside its own timeout.
    ///
    /// This is the exact shape of a routine `verify[]` entry (`./server &`, `npm run dev &`,
    /// anything that daemonises): `child.wait()` resolves at once because the DIRECT child is gone,
    /// but `read_to_end` on the pipes sees EOF only when the LAST write end closes — which is the
    /// descendant's whole lifetime. Awaiting the drain tasks outside the deadline therefore hung
    /// `run_verify_commands` (sequential, no outer timeout) forever, silently ignoring `timeoutMs`.
    /// Upstream is always bounded: its `"close"` event has the same stdio-EOF requirement, so it is
    /// `abortVerification`'s `finish({status: "timed-out", …})` that resolves the promise at
    /// `timeoutMs + 1000ms` (`acceptance.ts:742-759` @v0.34.0), and this asserts the same verdict on
    /// the same schedule.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_verify_command_that_daemonizes_still_returns_within_its_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let descendant_pid_file = dir.path().join("descendant");
        let started = tokio::time::Instant::now();

        // The outer guard turns "hangs forever" into a real, reportable failure instead of a wedged
        // test binary — the pre-fix code never returned here at all.
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_one_verify_command(
                &vc_timeout(
                    "sleep 300 & echo $! > descendant; exit 0",
                    Duration::from_millis(200),
                ),
                dir.path(),
            ),
        )
        .await
        .expect(
            "run_one_verify_command must honor its own timeout even when a backgrounded \
             grandchild still holds the stdout/stderr pipe — an unbounded post-wait drain hangs \
             acceptance evaluation with no error, no log line and no kill",
        );

        assert!(
            !passed(&result),
            "a command whose pipes outlive its own timeout is reported as a timeout, exactly as \
             upstream's abortVerification does, never as a pass"
        );
        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::TimedOut);
        let error = result.stderr.clone().unwrap_or_default();
        assert_eq!(
            error,
            crate::exec::acceptance::model::VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE,
            "the timed-out verdict must say so, and say WHICH timeout shape it was"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the call must return on its own deadline (200ms + the 1000ms hard-kill grace), not \
             the descendant's lifetime, got {:?}",
            started.elapsed()
        );

        // The descendant is deliberately NOT killed by the production path (upstream leaves a
        // deliberately daemonised process alone), so this test cleans up its own `sleep 300`.
        let pid = wait_for_published_pid(&descendant_pid_file, Duration::from_secs(5)).await;
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }


    /// The hard `SIGKILL` rung really fires: a command that traps and ignores `SIGTERM` cannot be
    /// stopped by upstream's first `child.kill(\"SIGTERM\")`, only by the 1000 ms-later
    /// `child.kill(\"SIGKILL\")` (`acceptance.ts:742-758`). Also pins that this function does not
    /// return until the process is CONFIRMED reaped, so the elapsed time covers the hard-kill
    /// grace period rather than returning early and leaving the kill in flight.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timed_out_verify_command_that_ignores_sigterm_is_sigkilled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let started = tokio::time::Instant::now();
        let result = run_one_verify_command(
            &vc_timeout(
                "trap '' TERM; echo $$ > pid; while true; do sleep 1; done",
                Duration::from_millis(200),
            ),
            dir.path(),
        )
        .await;
        let elapsed = started.elapsed();

        assert!(!passed(&result));
        assert!(
            elapsed >= crate::spawn::signal::TIMEOUT_SIGTERM_GRACE,
            "a SIGTERM-ignoring command must cost the full hard-kill grace period before the \
             SIGKILL rung fires, got {elapsed:?}"
        );
        let pid = wait_for_published_pid(&pid_file, Duration::from_secs(5)).await;
        assert!(
            wait_for_pid_gone(pid, Duration::from_secs(5)).await,
            "a SIGTERM-ignoring verify command pid {pid} must still be gone — SIGKILL cannot be \
             trapped"
        );
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_declared_cwd_resolves_against_the_run_level_cwd() {
        // `command.cwd ? path.resolve(defaultCwd, command.cwd) : defaultCwd` (`acceptance.ts:1137`).
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("sub")).expect("subdir");
        std::fs::write(dir.path().join("sub/marker.txt"), "hi").expect("seed file");

        let declared = VerifyCommand {
            cwd: Some("sub".to_string()),
            ..VerifyCommand::shell("test -f marker.txt")
        };
        let result = run_one_verify_command(&declared, dir.path()).await;
        assert!(
            passed(&result),
            "a declared relative cwd must resolve against the run-level cwd, got {result:?}"
        );

        // The same command WITHOUT the declared cwd must fail, proving the pass above came from
        // the declared `cwd` and not from the file being visible at the run-level cwd anyway.
        let undeclared = run_one_verify_command(&vc("test -f marker.txt"), dir.path()).await;
        assert!(!passed(&undeclared));
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn declared_env_is_layered_over_the_inherited_environment() {
        // `env: { ...process.env, ...(command.env ?? {}) }` (`acceptance.ts:724`) — the declared
        // pairs are added to the inherited environment, never a replacement of it.
        let dir = tempfile::tempdir().expect("tempdir");
        let declared = VerifyCommand {
            env: Some([("CYRUP_VERIFY_MARKER".to_string(), "1".to_string())].into()),
            ..VerifyCommand::shell(r#"test "$CYRUP_VERIFY_MARKER" = 1 && test -n "$PATH""#)
        };
        let result = run_one_verify_command(&declared, dir.path()).await;
        assert!(
            passed(&result),
            "the declared env pair must be present AND the inherited PATH must survive, got \
             {result:?}"
        );

        let undeclared =
            run_one_verify_command(&vc(r#"test "$CYRUP_VERIFY_MARKER" = 1"#), dir.path()).await;
        assert!(!passed(&undeclared), "the marker must come from the declared env, not the harness");
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_declared_timeout_ms_bounds_the_command_instead_of_the_default() {
        // `setTimeout(abortVerification, command.timeoutMs ?? 120_000)` (`acceptance.ts:759`).
        let dir = tempfile::tempdir().expect("tempdir");
        let declared = VerifyCommand {
            timeout_ms: Some(150),
            ..VerifyCommand::shell("sleep 30")
        };
        let started = std::time::Instant::now();
        let result = run_one_verify_command(&declared, dir.path()).await;
        let elapsed = started.elapsed();

        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::TimedOut);
        assert!(
            elapsed < Duration::from_secs(20),
            "a declared 150ms timeoutMs must bound the command, not DEFAULT_VERIFY_TIMEOUT — took \
             {elapsed:?}"
        );
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allow_failure_maps_a_nonzero_exit_to_allowed_failure_and_does_not_reject() {
        // `status: … passed ? "passed" : command.allowFailure ? "allowed-failure" : "failed"`
        // (`acceptance.ts:1193`) and `evaluateAcceptance`'s reject test, which never names
        // `allowed-failure` (`acceptance.ts:1297`).
        let dir = tempfile::tempdir().expect("tempdir");
        let declared = VerifyCommand {
            allow_failure: Some(true),
            ..VerifyCommand::shell("exit 1")
        };
        let result = run_one_verify_command(&declared, dir.path()).await;

        assert_eq!(result.exit_code, Some(1), "the real exit code is still observed");
        assert!(!passed(&result), "the raw exit observation stays false");
        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::AllowedFailure);
        assert!(!result.rejects(), "an allowed-failure command must not reject the run");
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allow_failure_never_rescues_a_timed_out_command() {
        // Upstream tests `timedOut` FIRST in the status ternary (`acceptance.ts:1193`), so a
        // command that hangs is `"timed-out"` — which `evaluateAcceptance` DOES reject — even when
        // it declared `allowFailure: true`.
        let dir = tempfile::tempdir().expect("tempdir");
        let declared = VerifyCommand {
            allow_failure: Some(true),
            timeout_ms: Some(150),
            ..VerifyCommand::shell("sleep 30")
        };
        let result = run_one_verify_command(&declared, dir.path()).await;

        assert_eq!(result.status, crate::exec::acceptance::model::VerifyRunStatus::TimedOut);
        assert!(result.rejects(), "a timeout rejects regardless of allowFailure");
    }


    /// Inference tops out at `Checked` now that `reviewed` is not a level — the escalation lives on
    /// the contract's `review` gate instead. A risky/write task must therefore still arm a REAL
    /// gate, not silently drop to a weaker one.
    #[test]
    fn heuristic_inference_tops_out_at_checked_with_a_required_review_gate() {
        let contract = AcceptanceContract::heuristic_default("worker", "Prepare the security release");
        assert_eq!(contract.required_level, AcceptanceStatus::Checked);
        assert!(matches!(
            &contract.review,
            Some(crate::exec::acceptance::model::ReviewSetting::Gate(gate)) if gate.required == Some(true)
                && gate.agent.as_deref() == Some("reviewer")
        ));
    }

}

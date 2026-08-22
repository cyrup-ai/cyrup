//! The provenance lattice itself: [`AcceptanceStatus`]'s strictly ordered levels and the
//! [`AcceptanceLedger`] that records which one a run reached.

#[cfg(test)]
pub(crate) mod testsupport;

pub mod contract;
pub mod gate;
pub mod inject;
pub mod lowering;
pub mod post_hoc;
pub mod report_source;
pub mod verify;

use self::contract::AcceptanceContract;

// ============================================================================================
// The ordered provenance lattice (func-SA §4.3, arch-SA §3.4)
// ============================================================================================

/// The acceptance-provenance ledger's status, on a **strictly ordered lattice**:
/// `NotRequired < Claimed < Attested < Checked < Verified < Reviewed`, with `Rejected` as a
/// distinct sink outside the ordered climb (func-SA §4.3: "sink `rejected`" — a rejected run does
/// not compare as "less than `NotRequired`"; it is simply excluded from the climb altogether and
/// handled as its own terminal outcome by [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`]).
///
/// `PartialOrd`/`Ord` are derived directly from declaration order below — this is exactly why the
/// lattice is expressed as a plain enum rather than, say, a wrapped `u8`: reordering these
/// variants would silently change the lattice's own comparison semantics, so the order here IS
/// the normative ordering func-SA §4.3 and arch-SA §3.4 both specify, not merely a convenient
/// listing. `Rejected` is declared last so it compares greater than every real level under the
/// derived `Ord` — callers needing lattice-position semantics (e.g. "does this satisfy at least
/// `Checked`") MUST use [`AcceptanceStatus::satisfies`] rather than raw `>=` comparison against
/// `Rejected`, since `Rejected` is a sink, not "the highest achieved level" (see that method's own
/// doc comment for the precise rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceStatus {
    /// No acceptance contract applies to this task at all — the default when no explicit
    /// `acceptance` param was given and heuristic inference did not resolve one either.
    NotRequired,
    /// The child's own prose asserts success (an `acceptance-report` block was extracted), with
    /// no independent evidence beyond that assertion. This is DI-SA-5's explicit "self-report" —
    /// the ledger's very existence is to distinguish this level from everything above it.
    Claimed,
    /// The child's `acceptance-report` block itself carries structured supporting fields
    /// (`changedFiles`, `testsAddedOrUpdated`, `commandsRun`, …) beyond a bare claim, but those
    /// fields are still the child's own self-report — still not independently checked.
    Attested,
    /// The orchestrator performed a structural/evidence check against observable, orchestrator-
    /// side state (e.g. the completion-mutation guard's own transcript-derived
    /// `attempted_mutation` signal, R-SA-034) — the first level backed by something the
    /// orchestrator itself observed, not merely something the child asserted.
    Checked,
    /// The orchestrator ACTUALLY EXECUTED every declared `verify[]` command as a real OS
    /// subprocess and every one exited 0 (DI-SA-5, R-SA-032). A child's own prose claim that it
    /// "ran the tests and they passed" NEVER produces this level — see [`crate::exec::acceptance::lattice::verify::run_verify_commands`].
    Verified,
    /// A genuine, externally-supplied independent reviewer result is present (func-SA §9 open
    /// question 10: the dispatch mechanism that produces this is out of scope for this module;
    /// this level is only ever reachable by a caller that already has a [`crate::exec::acceptance::lattice::contract::ReviewerResult`] in
    /// hand).
    Reviewed,
    /// The gate rejected the run: either an explicit contract's required level could not be
    /// reached, or a `verify[]` command genuinely failed. A sink, not a lattice position — see
    /// this enum's own doc comment and [`AcceptanceStatus::satisfies`].
    Rejected,
}

impl AcceptanceStatus {
    /// Whether this status satisfies (reaches or exceeds) `required` on the ordered lattice.
    /// `Rejected` NEVER satisfies any `required` level (including [`AcceptanceStatus::NotRequired`]) —
    /// it is a terminal failure outcome, not "the highest level, therefore satisfies everything",
    /// which the raw derived `Ord`'s `Rejected > Reviewed` relation would otherwise incorrectly
    /// imply if a caller used `>=` directly. This is the one piece of lattice logic that is NOT
    /// simply "derived `Ord`", which is exactly why this method exists rather than asking callers
    /// to compare `AcceptanceStatus` values directly.
    #[must_use]
    pub fn satisfies(self, required: AcceptanceStatus) -> bool {
        if self == AcceptanceStatus::Rejected {
            return false;
        }
        if required == AcceptanceStatus::Rejected {
            // Nothing "requires" Rejected — Rejected is never a valid target level for a
            // contract to declare; treat it as unsatisfiable by construction.
            return false;
        }
        self >= required
    }

    /// This lattice position as upstream's [`crate::exec::acceptance::model::AcceptanceEvidenceStatus`]
    /// (`shared/types.ts:770-778` @v0.43.0) — the projection [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] uses to fill
    /// [`AcceptanceLedger::evidence_status`].
    ///
    /// The two vocabularies agree member-for-member on everything the EVIDENCE rungs can produce.
    /// The two that cannot appear there are mapped rather than invented: `Reviewed` is a review
    /// outcome, and upstream's `evidenceStatus` never takes it (its review block moves `status`
    /// alone, `acceptance.ts:1318-1336`), so it projects onto the highest evidence level a reviewed
    /// run must already have held — `Verified`; `Rejected` maps to `Rejected`, which upstream's
    /// evidence vocabulary does have.
    #[must_use]
    pub fn evidence_status(self) -> crate::exec::acceptance::model::AcceptanceEvidenceStatus {
        match self {
            AcceptanceStatus::NotRequired => crate::exec::acceptance::model::AcceptanceEvidenceStatus::NotRequired,
            AcceptanceStatus::Claimed => crate::exec::acceptance::model::AcceptanceEvidenceStatus::Claimed,
            AcceptanceStatus::Attested => crate::exec::acceptance::model::AcceptanceEvidenceStatus::Attested,
            AcceptanceStatus::Checked => crate::exec::acceptance::model::AcceptanceEvidenceStatus::Checked,
            AcceptanceStatus::Verified | AcceptanceStatus::Reviewed => {
                crate::exec::acceptance::model::AcceptanceEvidenceStatus::Verified
            }
            AcceptanceStatus::Rejected => crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
        }
    }
}

/// The full acceptance-provenance record attached to one run's result (func-SA §4.3's
/// `AcceptanceLedger`, arch-SA §3.4).
///
/// # Relationship to [`crate::exec::acceptance::model::AcceptanceLedger`]
///
/// Upstream has exactly ONE ledger type (`AcceptanceLedger`, `shared/types.ts:785-800` @v0.43.0)
/// produced by exactly one gate. This crate still has two, and that is unfinished port work rather
/// than a design: `status`/`detail` here are the enum-lattice verdict this crate computes, while
/// [`crate::exec::acceptance::model::AcceptanceLedger`] is the faithful upstream shape. The two are CONVERGING — the
/// evidence-bearing fields below are now the upstream types verbatim, so nothing upstream records
/// is lost on the live path any more, and what remains is the verdict projection itself. See
/// [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`]'s doc comment for the precise remaining delta.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptanceLedger {
    /// The achieved status.
    pub status: AcceptanceStatus,
    /// G78 — upstream's `AcceptanceLedger.evidenceStatus` (`shared/types.ts:787` @v0.43.0), the
    /// field v0.43.0 split OUT of `status` so that an independent review that has not happened yet
    /// cannot erase the evidence level the run already earned: a `verified` run awaiting a reviewer
    /// reads `status: review-required, evidenceStatus: verified`, where before the split it read
    /// `verified` or `review-required` but never both.
    ///
    /// It moves in lockstep with `status` through the attestation/checked/verified rungs and is
    /// then FROZEN — `evaluateAcceptance`'s review block (`acceptance.ts:1318-1336`) rewrites only
    /// `status`. This is why the review rung below never touches it.
    #[serde(default = "default_pending_evidence_status")]
    pub evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus,
    /// A human-readable detail string: which check ran, which `verify[]` command failed and why,
    /// or `None` when `status` speaks for itself (e.g. `NotRequired`).
    pub detail: Option<String>,
    /// The individual `verify[]` command outcomes that contributed to `status`, in declaration
    /// order — present whenever [`crate::exec::acceptance::lattice::verify::run_verify_commands`] actually ran (i.e. the contract's
    /// required level was `Verified` or above), empty otherwise. Kept on the ledger itself (not
    /// merely folded into `detail`'s free text) so a caller/UI can render per-command
    /// pass/fail without re-parsing `detail`.
    ///
    /// G80 — these are upstream's own `AcceptanceLedger.verifyRuns[]` entries
    /// ([`crate::exec::acceptance::model::AcceptanceVerifyResult`], `shared/types.ts:736-758`), which is what carries the
    /// memoization evidence upstream stamps on every one of them: `artifactPath`, `cacheKey`,
    /// `memoized`, `envKeys`, `envHash`, `workspaceState` and `artifactError`
    /// (`acceptance.ts:1106,1112,1128-1129`). Until this field took the upstream type, the live
    /// foreground gate wrote a crate-local `VerifyCommandResult` that had nowhere to put any of
    /// them, so every one was silently dropped in production.
    #[serde(default)]
    pub verify_results: Vec<crate::exec::acceptance::model::AcceptanceVerifyResult>,
}

/// `serde(default)` for [`AcceptanceLedger::evidence_status`]: a ledger persisted before the field
/// existed recorded no evidence status, and `pending` is upstream's own name for exactly that
/// (`AcceptanceEvidenceStatus` includes `"pending"`, `shared/types.ts:770-777`). Deserializing such a
/// ledger as `not-required` would instead assert something the record never claimed.
fn default_pending_evidence_status() -> crate::exec::acceptance::model::AcceptanceEvidenceStatus {
    crate::exec::acceptance::model::AcceptanceEvidenceStatus::Pending
}

impl AcceptanceLedger {
    /// Construct a `NotRequired` ledger with no detail — the default outcome when no contract
    /// resolves for a task at all.
    #[must_use]
    pub fn not_required() -> Self {
        Self {
            status: AcceptanceStatus::NotRequired,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::NotRequired,
            detail: None,
            verify_results: Vec::new(),
        }
    }
}

/// Build the acceptance ledger for a run that TIMED OUT before acceptance could be evaluated (pi
/// `buildTimedOutAcceptanceLedger`, `execution.ts:101-113`, applied at `execution.ts:1089-1090`).
///
/// A timed-out run whose contract required an acceptance level is [`AcceptanceStatus::Rejected`] —
/// NOT the [`AcceptanceStatus::NotRequired`] a non-clean gate would otherwise yield from
/// [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`] (which short-circuits on `!gate.is_clean()`) — with a failed timeout
/// runtime check recorded in `detail` ("Acceptance was not evaluated because the subagent timed
/// out.", pi's `runtimeChecks[0].message`). A contract that required no acceptance at all
/// (`required_level == NotRequired`, pi's `acceptance.level === "none"`) stays `NotRequired` with no
/// detail, mirroring pi's `acceptance.level === "none" ? "not-required" : "rejected"` branch.
#[must_use]
pub fn build_timed_out_acceptance_ledger(contract: &AcceptanceContract) -> AcceptanceLedger {
    if contract.required_level == AcceptanceStatus::NotRequired {
        AcceptanceLedger::not_required()
    } else {
        AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            // `buildSkippedAcceptanceLedger` writes ONE `status` to both fields
            // (`acceptance.ts:1342-1345` @v0.43.0).
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("Acceptance was not evaluated because the subagent timed out.".to_string()),
            verify_results: Vec::new(),
        }
    }
}

impl AcceptanceStatus {
    /// The wire/prose string for this level, matching [`AcceptanceStatus`]'s own `serde`
    /// `kebab-case` rendering (`"not-required"`, `"claimed"`, …) — used by
    /// [`crate::exec::acceptance::lattice::inject::inject_acceptance_contract`] so the text shown to the child matches exactly what this
    /// module itself would parse back, and by [`AcceptanceLedger`] consumers that want the same
    /// string without going through `serde_json`.
    #[must_use]
    pub fn as_wire_str(self) -> &'static str {
        match self {
            AcceptanceStatus::NotRequired => "not-required",
            AcceptanceStatus::Claimed => "claimed",
            AcceptanceStatus::Attested => "attested",
            AcceptanceStatus::Checked => "checked",
            AcceptanceStatus::Verified => "verified",
            AcceptanceStatus::Reviewed => "reviewed",
            AcceptanceStatus::Rejected => "rejected",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;


    // ---------------------------------------------------------------------------------------
    // AcceptanceStatus: lattice ordering and satisfies()
    // ---------------------------------------------------------------------------------------

    #[test]
    fn lattice_is_strictly_ordered_not_required_through_reviewed() {
        assert!(AcceptanceStatus::NotRequired < AcceptanceStatus::Claimed);
        assert!(AcceptanceStatus::Claimed < AcceptanceStatus::Attested);
        assert!(AcceptanceStatus::Attested < AcceptanceStatus::Checked);
        assert!(AcceptanceStatus::Checked < AcceptanceStatus::Verified);
        assert!(AcceptanceStatus::Verified < AcceptanceStatus::Reviewed);
    }


    #[test]
    fn satisfies_is_reflexive_and_monotone() {
        assert!(AcceptanceStatus::Verified.satisfies(AcceptanceStatus::Verified));
        assert!(AcceptanceStatus::Reviewed.satisfies(AcceptanceStatus::Checked));
        assert!(!AcceptanceStatus::Checked.satisfies(AcceptanceStatus::Verified));
        assert!(AcceptanceStatus::Claimed.satisfies(AcceptanceStatus::NotRequired));
    }


    #[test]
    fn rejected_never_satisfies_anything_despite_deriving_greatest_ord() {
        // Rejected is declared LAST so raw Ord would say Rejected > Reviewed > ... — satisfies()
        // must explicitly override that so Rejected never counts as "reaching" any real level.
        assert!(AcceptanceStatus::Rejected > AcceptanceStatus::Reviewed);
        assert!(!AcceptanceStatus::Rejected.satisfies(AcceptanceStatus::NotRequired));
        assert!(!AcceptanceStatus::Rejected.satisfies(AcceptanceStatus::Verified));
    }


    #[test]
    fn wire_strings_round_trip_through_serde_kebab_case() {
        for status in [
            AcceptanceStatus::NotRequired,
            AcceptanceStatus::Claimed,
            AcceptanceStatus::Attested,
            AcceptanceStatus::Checked,
            AcceptanceStatus::Verified,
            AcceptanceStatus::Reviewed,
            AcceptanceStatus::Rejected,
        ] {
            let json = serde_json::to_string(&status).expect("serializes");
            assert_eq!(json, format!("\"{}\"", status.as_wire_str()));
        }
    }

}

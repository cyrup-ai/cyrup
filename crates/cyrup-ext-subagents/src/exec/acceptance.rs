//! The acceptance-provenance ledger: contract injection, gate evaluation, and REAL `verify[]`
//! command execution (func-SA §5.2 R-SA-023/030/032/033; arch-SA §6.3.4; DI-SA-5).
//!
//! # Scope
//!
//! This module owns four things, in the order a caller (a later phase's `exec/mod.rs::run_sync`
//! completion path, per R-SA-033's fixed ordering) is expected to use them:
//!
//! 1. [`AcceptanceStatus`] / [`AcceptanceLedger`] — the strictly ordered provenance lattice
//!    (`not-required < claimed < attested < checked < verified < reviewed`, sink `rejected`)
//!    func-SA §4.3 and arch-SA §3.4 both name. `PartialOrd`/`Ord` are derived directly off
//!    declaration order, which is why variant order below is itself normative, not cosmetic.
//! 2. [`AcceptanceContract`] + [`inject_acceptance_contract`] (R-SA-023) — resolving the
//!    effective contract for a task (explicit param, or a heuristic inferred default) and
//!    appending a machine-parseable `## Acceptance Contract` block to the task text sent to the
//!    child, instructing it to emit a fenced `acceptance-report` JSON block as the final part of
//!    its answer. This MUST run *before* [`crate::spawn::SpawnedChild::spawn`] — it is pure task-
//!    text augmentation, not itself a spawn.
//! 3. [`evaluate_acceptance`] (R-SA-032, DI-SA-5) — the gate itself. For `checked` and above,
//!    structural/evidence checks run against the extracted output and observed transcript
//!    (`exec/completion_guard.rs`'s already-computed [`crate::exec::completion_guard::CompletionMutationGuardResult`]
//!    is one such piece of evidence this function consumes, never re-derives). For `verified`,
//!    [`run_verify_commands`] is invoked to ACTUALLY EXECUTE every declared `verify[]` command as
//!    a real OS subprocess and observe its real exit code — **a child's own prose claim that a
//!    command succeeded is never sufficient and is never even consulted here**; the whole point
//!    of this module, per DI-SA-5, is that this ledger is orchestrator-observed, not self-
//!    reported. For `reviewed`, an externally-supplied [`ReviewerResult`] MUST already be present
//!    on the [`AcceptanceContract`] — this module has no reviewer-dispatch mechanism of its own
//!    (func-SA §9 open question 10: "who supplies the `reviewed`-level reviewer result" is
//!    unresolved upstream and explicitly out of scope here; this module only *consumes* one if a
//!    caller already has it).
//! 4. [`apply_post_hoc_correction`] (R-SA-033) — the strictly-ordered exit-code/error rewrite this
//!    module's own evaluation feeds into, applied only when the gate rejects an otherwise-clean,
//!    explicit-contract run.
//!
//! # Ordering this module does NOT own but MUST be called consistently with (R-SA-033)
//!
//! `structured-output validation (R-SA-030, exec/output.rs) -> completion-mutation guard
//! (R-SA-034, exec/completion_guard.rs) -> acceptance-gate evaluation (this module, gated on
//! `exit_code == 0 && !detached && !interrupted && !timed_out`) -> result compaction (R-SA-043,
//! a later phase)`. This module's [`evaluate_acceptance`] takes that same four-flag gate as an
//! explicit, mandatory parameter — see that function's doc comment — precisely so a caller cannot
//! accidentally invoke it before confirming the run is otherwise clean; the gate condition is
//! re-asserted defensively inside the function itself (never trusted purely to caller discipline)
//! since a wrongly-run acceptance check on a non-clean run is exactly the kind of correctness bug
//! R-SA-033's ordering exists to prevent.
//!
//! # Why `verify[]` execution is a bespoke `tokio::process::Command` call, not `SpawnedChild`
//!
//! [`crate::spawn::SpawnedChild`] spawns *the `cyrup` binary itself* (re-exec'd, per func-SA
//! §1.1's mandated mechanism) — that is the mechanism for launching a **subagent**. A `verify[]`
//! entry is an arbitrary shell command (`cargo test`, `npm run lint`, …), not a subagent
//! invocation, so it is not spawned through `SpawnedChild` at all. It is, however, still governed
//! by the *same underlying mandate* (DI-SA-1: "every subagent execution — foreground, background,
//! nested, acceptance-`verify[]` — is a genuine OS subprocess") and R-SA-032's explicit "via the
//! same subprocess mechanism as any other command execution available to this extension" — i.e.
//! a REAL `tokio::process::Command` child, never an in-process shell-out shim or a simulated exit
//! code. This mirrors `spawn::worktree`'s own precedent exactly (that module's real `git`
//! subprocess calls, module doc: "this shells out to a real subprocess, never a library" — this
//! module applies the identical reasoning to `verify[]` commands). This crate has zero dependency
//! on `cyrup-tools` (arch-SA §7's dependency table), so [`run_verify_commands`] does not reuse
//! that crate's own `ops::local` shell primitive either — it is a small, self-contained, real
//! subprocess call built directly on `tokio::process::Command`, exactly like `spawn::worktree`'s
//! `git` invocations.
//!
//! This module has ZERO dependency on `cyrup-agent` — every message/content shape it inspects is
//! the same opaque `serde_json::Value`-backed [`crate::exec::ndjson::SubagentEvent`] already
//! exposes (arch-SA §2.1/§1.1, restated at every module boundary in this crate).

use std::path::Path;

use crate::exec::completion_guard::CompletionMutationGuardResult;
use crate::exec::output::looks_like_acceptance_report;
use crate::spawn::signal::TIMEOUT_SIGTERM_GRACE;

// ============================================================================================
// The ordered provenance lattice (func-SA §4.3, arch-SA §3.4)
// ============================================================================================

/// The acceptance-provenance ledger's status, on a **strictly ordered lattice**:
/// `NotRequired < Claimed < Attested < Checked < Verified < Reviewed`, with `Rejected` as a
/// distinct sink outside the ordered climb (func-SA §4.3: "sink `rejected`" — a rejected run does
/// not compare as "less than `NotRequired`"; it is simply excluded from the climb altogether and
/// handled as its own terminal outcome by [`evaluate_acceptance`]).
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
    /// "ran the tests and they passed" NEVER produces this level — see [`run_verify_commands`].
    Verified,
    /// A genuine, externally-supplied independent reviewer result is present (func-SA §9 open
    /// question 10: the dispatch mechanism that produces this is out of scope for this module;
    /// this level is only ever reachable by a caller that already has a [`ReviewerResult`] in
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

    /// This lattice position as upstream's [`model::AcceptanceEvidenceStatus`]
    /// (`shared/types.ts:770-778` @v0.43.0) — the projection [`evaluate_acceptance`] uses to fill
    /// [`AcceptanceLedger::evidence_status`].
    ///
    /// The two vocabularies agree member-for-member on everything the EVIDENCE rungs can produce.
    /// The two that cannot appear there are mapped rather than invented: `Reviewed` is a review
    /// outcome, and upstream's `evidenceStatus` never takes it (its review block moves `status`
    /// alone, `acceptance.ts:1318-1336`), so it projects onto the highest evidence level a reviewed
    /// run must already have held — `Verified`; `Rejected` maps to `Rejected`, which upstream's
    /// evidence vocabulary does have.
    #[must_use]
    pub fn evidence_status(self) -> model::AcceptanceEvidenceStatus {
        match self {
            AcceptanceStatus::NotRequired => model::AcceptanceEvidenceStatus::NotRequired,
            AcceptanceStatus::Claimed => model::AcceptanceEvidenceStatus::Claimed,
            AcceptanceStatus::Attested => model::AcceptanceEvidenceStatus::Attested,
            AcceptanceStatus::Checked => model::AcceptanceEvidenceStatus::Checked,
            AcceptanceStatus::Verified | AcceptanceStatus::Reviewed => {
                model::AcceptanceEvidenceStatus::Verified
            }
            AcceptanceStatus::Rejected => model::AcceptanceEvidenceStatus::Rejected,
        }
    }
}

/// The full acceptance-provenance record attached to one run's result (func-SA §4.3's
/// `AcceptanceLedger`, arch-SA §3.4).
///
/// # Relationship to [`model::AcceptanceLedger`]
///
/// Upstream has exactly ONE ledger type (`AcceptanceLedger`, `shared/types.ts:785-800` @v0.43.0)
/// produced by exactly one gate. This crate still has two, and that is unfinished port work rather
/// than a design: `status`/`detail` here are the enum-lattice verdict this crate computes, while
/// [`model::AcceptanceLedger`] is the faithful upstream shape. The two are CONVERGING — the
/// evidence-bearing fields below are now the upstream types verbatim, so nothing upstream records
/// is lost on the live path any more, and what remains is the verdict projection itself. See
/// [`evaluate_acceptance`]'s doc comment for the precise remaining delta.
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
    pub evidence_status: model::AcceptanceEvidenceStatus,
    /// A human-readable detail string: which check ran, which `verify[]` command failed and why,
    /// or `None` when `status` speaks for itself (e.g. `NotRequired`).
    pub detail: Option<String>,
    /// The individual `verify[]` command outcomes that contributed to `status`, in declaration
    /// order — present whenever [`run_verify_commands`] actually ran (i.e. the contract's
    /// required level was `Verified` or above), empty otherwise. Kept on the ledger itself (not
    /// merely folded into `detail`'s free text) so a caller/UI can render per-command
    /// pass/fail without re-parsing `detail`.
    ///
    /// G80 — these are upstream's own `AcceptanceLedger.verifyRuns[]` entries
    /// ([`model::AcceptanceVerifyResult`], `shared/types.ts:736-758`), which is what carries the
    /// memoization evidence upstream stamps on every one of them: `artifactPath`, `cacheKey`,
    /// `memoized`, `envKeys`, `envHash`, `workspaceState` and `artifactError`
    /// (`acceptance.ts:1106,1112,1128-1129`). Until this field took the upstream type, the live
    /// foreground gate wrote a crate-local `VerifyCommandResult` that had nowhere to put any of
    /// them, so every one was silently dropped in production.
    #[serde(default)]
    pub verify_results: Vec<model::AcceptanceVerifyResult>,
}

/// `serde(default)` for [`AcceptanceLedger::evidence_status`]: a ledger persisted before the field
/// existed recorded no evidence status, and `pending` is upstream's own name for exactly that
/// (`AcceptanceEvidenceStatus` includes `"pending"`, `shared/types.ts:770-777`). Deserializing such a
/// ledger as `not-required` would instead assert something the record never claimed.
fn default_pending_evidence_status() -> model::AcceptanceEvidenceStatus {
    model::AcceptanceEvidenceStatus::Pending
}

impl AcceptanceLedger {
    /// Construct a `NotRequired` ledger with no detail — the default outcome when no contract
    /// resolves for a task at all.
    #[must_use]
    pub fn not_required() -> Self {
        Self {
            status: AcceptanceStatus::NotRequired,
            evidence_status: model::AcceptanceEvidenceStatus::NotRequired,
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
/// [`evaluate_acceptance`] (which short-circuits on `!gate.is_clean()`) — with a failed timeout
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
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("Acceptance was not evaluated because the subagent timed out.".to_string()),
            verify_results: Vec::new(),
        }
    }
}

// ============================================================================================
// R-SA-023: Acceptance contract injection
// ============================================================================================

/// One declared verification command (func-SA `acceptance.verify[]`): a literal shell command
/// string plus the per-command execution knobs upstream's `AcceptanceVerifyCommand` carries,
/// executed via a real subprocess (R-SA-032, [`run_verify_commands`]) — never parsed or
/// interpreted beyond being handed to a shell, and never treated as satisfied by anything the
/// child itself claims about it.
///
/// This is deliberately an alias for [`model::AcceptanceVerifyCommand`] — the faithful port of
/// upstream `AcceptanceVerifyCommand` (`pi-subagents/src/runs/shared/types.ts` @v0.43.0, whose
/// accepted key set upstream pins as `ACCEPTANCE_VERIFY_KEYS = {id, command, timeoutMs, cwd, env,
/// allowFailure}`, `acceptance.ts:52`) — rather than a second, parallel struct. There is likewise
/// now only ONE runner consuming it ([`model::run_verify_command`], via
/// [`model::run_memoized_verify_command`]), so a declared command has exactly one meaning. Before SUBA-C12b this alias was a bare `String`, so
/// [`lower_acceptance_input`] validated all six keys (`model::validate_verify_input`) and then
/// discarded five of them: `cwd`/`env`/`timeoutMs` never reached the subprocess and `allowFailure`
/// never reached the gate.
pub type VerifyCommand = model::AcceptanceVerifyCommand;

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
    /// [`evaluate_acceptance`] simply never executes them unless the required level demands it).
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
    /// flag existed, [`lower_acceptance_input`] mapped a bare `"none"` onto an explicit
    /// `NotRequired` contract whose [`is_no_op`](Self::is_no_op) short-circuited the whole gate,
    /// so a one-word policy silently disarmed acceptance that pi still enforces.
    ///
    /// Only ever consulted by [`AcceptanceContract::resolve_effective`]; a contract this crate
    /// builds directly in Rust (rather than lowering from a wire policy) keeps the historical
    /// "an explicit `NotRequired` means the caller wants no gate" reading — see
    /// [`AcceptanceContract::explicit`].
    pub disables_gate: bool,
    /// The resolved acceptance criteria this run is gated on (pi `ResolvedAcceptanceConfig.criteria`,
    /// `shared/types.ts:302-312` @v0.34.0), already through [`model::normalize_criteria`] so every entry
    /// carries an `id`, a non-blank `must` and a [`model::GateSeverity`].
    ///
    /// [`inject_acceptance_contract`] renders them as pi's `- <id>: <must>` list so the child KNOWS
    /// what it must report, and [`evaluate_acceptance`]'s `Checked` rung enforces them via
    /// [`model::check_criteria_satisfied`] — pi `evaluateAcceptance`, `acceptance.ts:1297-1321`. Empty
    /// means "nothing declared", which is enforcement-neutral: pi's own `formatAcceptancePrompt`
    /// falls back to `- Return the requested result.` and `checkCriteriaSatisfied` over an empty
    /// list yields no checks.
    pub criteria: Vec<model::ResolvedAcceptanceGate>,
    /// The evidence kinds the child's own `acceptance-report` block MUST carry (pi
    /// `ResolvedAcceptanceConfig.evidence`). Rendered into the prompt as pi's
    /// `Required evidence: <kinds>` line and enforced by [`model::run_structural_checks`] —
    /// including the REAL `git status --short` subprocess pi runs for
    /// [`model::AcceptanceEvidenceKind::NoStagedFiles`] (`checkNoStagedFiles`,
    /// `acceptance.ts:939-948`).
    pub evidence: Vec<model::AcceptanceEvidenceKind>,
    /// The declared review gate (pi `ResolvedAcceptanceConfig.review`). Rendered into the prompt as
    /// pi's `Review gate: required|optional[ by <agent>]` / `Review focus:` lines, and consulted at
    /// [`evaluate_acceptance`]'s `Reviewed` rung: a gate declaring `required: false` is pi's
    /// `optionalReview` (`acceptance.ts:1321-1335`), which does NOT reject a run that reached
    /// `Reviewed` with no [`ReviewerResult`] in hand.
    pub review: Option<model::ReviewSetting>,
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
    /// `acceptance` policy (builder-style), the four fields [`lower_acceptance_input`] used to
    /// validate and then DISCARD.
    ///
    /// Splitting this off [`AcceptanceContract::explicit`] keeps that constructor's existing
    /// signature (and its ~40 in-tree call sites) intact: a contract built in Rust with no declared
    /// policy is exactly the pre-existing shape, and only a wire-lowered policy carries the extra
    /// four. Criteria are expected to have already been through
    /// [`model::normalize_criteria`] — this setter does no normalization of its own so the single
    /// normalization rule stays in the [`model`] port that owns it.
    #[must_use]
    pub fn with_policy(
        mut self,
        criteria: Vec<model::ResolvedAcceptanceGate>,
        evidence: Vec<model::AcceptanceEvidenceKind>,
        review: Option<model::ReviewSetting>,
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
    /// [`model`] port's faithful line-for-line copy of it, so the four-way decision tree, the
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
    /// `acceptance-report` instruction at all: [`inject_acceptance_contract`] returns the task
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
    /// [`lower_acceptance_input`] on the declared policy (`spawn/chain_graph.rs`), so the
    /// `dynamic fanout context` escalation is reachable there rather than lost outright.
    #[must_use]
    pub fn heuristic_default(agent_local_name: &str, task: &str) -> Self {
        let inferred = model::resolve_effective_acceptance(&model::AcceptanceResolveInput {
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
                model::AcceptanceLevel::Verified => AcceptanceStatus::Verified,
                model::AcceptanceLevel::Checked => AcceptanceStatus::Checked,
                model::AcceptanceLevel::Attested
                | model::AcceptanceLevel::Auto
                | model::AcceptanceLevel::None => AcceptanceStatus::Attested,
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
    /// `explicit == None` is pi's `explicitLevel === "auto"` — [`lower_acceptance_input`] maps
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
    /// NotRequired` and no reviewer result is attached. [`evaluate_acceptance`] short-circuits on
    /// this so a task with no acceptance requirement never even inspects the transcript.
    #[must_use]
    pub fn is_no_op(&self) -> bool {
        self.required_level == AcceptanceStatus::NotRequired && self.reviewer_result.is_none()
    }

    /// Project this contract onto the [`model`] port's [`model::ResolvedAcceptanceConfig`], the
    /// shape pi's own `formatAcceptancePrompt` consumes (`acceptance.ts:304`) — so the child-facing
    /// prompt is rendered by the faithful port rather than by a second, drifting copy of it.
    ///
    /// The level mapping is total both ways for every level a policy can actually declare
    /// ([`lower_acceptance_input`] only ever produces `NotRequired`/`Attested`/`Checked`/`Verified`/
    /// `Reviewed`). The two [`AcceptanceStatus`] variants with no [`model::AcceptanceLevel`]
    /// counterpart are outcomes rather than requirements and are unreachable as a `required_level`:
    /// [`AcceptanceStatus::Claimed`] is the self-report floor [`evaluate_acceptance`] computes, and
    /// [`AcceptanceStatus::Rejected`] is clamped away by [`AcceptanceContract::explicit`]. Both are
    /// mapped to their nearest declarable neighbour rather than silently dropping the whole prompt.
    ///
    /// `inferred_reason` is empty because [`AcceptanceContract`] carries no reasons field of its
    /// own — [`AcceptanceContract::heuristic_default`] does now produce upstream's reason strings
    /// (via [`model::resolve_effective_acceptance`]) but drops them at this seam. The field is
    /// prompt-irrelevant (`formatAcceptancePrompt` never reads it); it surfaces upstream only on
    /// the LEDGER's `inferredReason`, which this crate's narrower [`AcceptanceLedger`] does not
    /// have either.
    #[must_use]
    pub fn to_resolved_config(&self) -> model::ResolvedAcceptanceConfig {
        let level = match self.required_level {
            AcceptanceStatus::NotRequired => model::AcceptanceLevel::None,
            AcceptanceStatus::Claimed | AcceptanceStatus::Attested => {
                model::AcceptanceLevel::Attested
            }
            AcceptanceStatus::Checked => model::AcceptanceLevel::Checked,
            AcceptanceStatus::Verified => model::AcceptanceLevel::Verified,
            // v0.43.0 deleted the `reviewed` LEVEL (`shared/types.ts:639`), so a lattice contract that
            // requires an independent reviewer has no level of its own to project onto. Upstream
            // expresses exactly that shape as `level: "checked"` plus `review.required` — which
            // this contract already carries separately in `review` — so `Reviewed` projects to
            // `Checked`, NOT to `Verified` (which would additionally demand `verify[]` commands
            // this contract never declared and reject the run for their absence,
            // `acceptance.ts:1281-1286`).
            AcceptanceStatus::Reviewed | AcceptanceStatus::Rejected => {
                model::AcceptanceLevel::Checked
            }
        };
        model::ResolvedAcceptanceConfig {
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

// ============================================================================================
// Lowering a raw wire `acceptance` value onto an `AcceptanceContract` (SUBA-041 / SUBA-N04)
// ============================================================================================

/// Lower a raw wire `acceptance` value (pi `AcceptanceOverride`, `schemas.ts:80-93`) onto this
/// crate's [`AcceptanceContract`], after running pi's own `validateAcceptanceInput`
/// (`pi-subagents/src/runs/shared/acceptance.ts:164-286` @v0.34.0, applied at
/// `subagent-executor.ts:1418`) so a malformed policy is refused BEFORE any child spawns, with pi's
/// verbatim messages.
///
/// Level mapping (pi `AcceptanceLevel` -> [`AcceptanceStatus`]): `auto` yields `None`, i.e. pi's own
/// "omitted means auto-inferred" — [`crate::exec::run_sync`] then resolves it through
/// [`AcceptanceContract::resolve_effective`] against
/// [`AcceptanceContract::heuristic_default`] (R-SA-023), which is this crate's `inferLevel`. Every
/// other level (and the `false` shorthand, pi's `level: "none"`) becomes an EXPLICIT contract, which
/// is what arms R-SA-033's post-hoc exit-code correction.
///
/// An explicit level is a **floor**, not a replacement ([`AcceptanceContract::explicit_floor`]):
/// upstream takes `max(explicit, inferred)` by rank (`acceptance.ts:277-281`), so the only inputs
/// that can lower or remove the inferred requirement are the two `explicitAcceptanceCanDisable`
/// accepts (`:134-136`) — `false`, and `{ level: "none", reason: <non-blank> }`. A bare `"none"`
/// string is NOT one of them.
///
/// # Why this lives here and not on one call site
///
/// It is the SINGLE lowering every execution surface shares: the SINGLE-mode `acceptance` tool param
/// (`extension.rs::route_single`, pi `subagent-executor.ts:1418`), and every chain/parallel/
/// background STEP's own `acceptance` (`background/runner_main.rs::ExecSingleStepExecutor::
/// run_single`, pi `chain-execution.ts:400,1335` — which pass `task.acceptance`/`seqStep.acceptance`
/// into the very same `runSync` call the single path uses). SUBA-N04: the step path used to hard-drop
/// the field to `None`, so a declared contract ran UNVERIFIED; a second parser would have re-opened
/// exactly that drift, so both paths call this one function.
///
/// `criteria`/`evidence`/`review`/`stopRules` are lowered onto the contract too (SUBA-C13), through
/// the [`model`] port's own `normalizeCriteria` (`acceptance.ts:330-342`), so
/// [`inject_acceptance_contract`] can TELL the child about them and [`evaluate_acceptance`]'s
/// `Checked` rung can ENFORCE them. Before SUBA-C13 all four were validated here and then dropped on
/// the floor: `{ level: "checked", criteria: [{ id: "c1", must: "add a regression test" }],
/// evidence: ["tests-added", "no-staged-files"] }` armed a gate that could never fire, because the
/// child was never told to report `c1` and nothing ever looked for `testsAddedOrUpdated` or ran
/// `git status`.
///
/// **[CYRUP-DELTA]** `reason` is still consumed only as the `explicitAcceptanceCanDisable` predicate
/// below, never carried onto the contract — it has no injection or enforcement role upstream either.
/// And the evidence set here is the DECLARED one only: pi additionally unions in
/// `requiredEvidenceForLevel(level)` when the resolved level differs from the inferred one
/// (`acceptance.ts:282-286`), which needs the [`model`] port's full `inferLevel` tree rather than
/// this crate's enum-lattice [`AcceptanceContract::heuristic_default`], so a bare
/// `{ level: "checked" }` with no `evidence` key still declares no evidence here where pi would
/// require five kinds.
///
/// # Errors
///
/// Returns every `validateAcceptanceInput` message, space-joined, exactly as pi renders them
/// (`subagent-executor.ts:1758-1762`).
pub fn lower_acceptance_input(
    raw: &serde_json::Value,
) -> Result<Option<AcceptanceContract>, String> {
    let errors = model::validate_acceptance_input(raw, "acceptance");
    if !errors.is_empty() {
        return Err(errors.join(" "));
    }

    fn level_to_status(level: &str) -> Option<AcceptanceStatus> {
        match level {
            "none" => Some(AcceptanceStatus::NotRequired),
            "attested" => Some(AcceptanceStatus::Attested),
            "checked" => Some(AcceptanceStatus::Checked),
            "verified" => Some(AcceptanceStatus::Verified),
            // `"reviewed"` is deliberately absent: it is not an `AcceptanceLevel` at v0.43.0 and
            // `model::validate_acceptance_input` has already rejected it above with
            // `EXPLICIT_REVIEWED_UNAVAILABLE`, so this arm can never be reached from the wire.
            // `"auto"` (and anything `validate_acceptance_input` let through) infers.
            _ => None,
        }
    }

    match raw {
        // pi `acceptance: false` is the `level: "none"` shorthand (`acceptance.ts:149-154`) — and
        // the ONE string-ish form that genuinely disables the gate, because
        // `normalizeAcceptanceInput` supplies the reason `"disabled by deprecated false shorthand"`
        // itself, satisfying `explicitAcceptanceCanDisable` (`:134-136`).
        serde_json::Value::Bool(false) => Ok(Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            Vec::new(),
        ))),
        // A bare level string carries no `reason`, so `explicitAcceptanceCanDisable` is false for
        // it (`acceptance.ts:149-174`): `"none"` here is a FLOOR of `none`, which
        // [`AcceptanceContract::resolve_effective`]'s max then discards in favour of the inferred
        // level — it does not switch the gate off.
        serde_json::Value::String(level) => Ok(level_to_status(level)
            .map(|status| AcceptanceContract::explicit_floor(status, Vec::new()))),
        serde_json::Value::Object(config) => {
            let verify: Vec<VerifyCommand> = config
                .get("verify")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| lower_verify_command(item, index))
                        .collect()
                })
                .unwrap_or_default();
            let level = config.get("level").and_then(serde_json::Value::as_str);
            // `explicitAcceptanceCanDisable` (`acceptance.ts:167-174`): only an object whose
            // `reason` is a non-blank string may turn the gate off. In practice
            // `validate_acceptance_input` already rejects `{ level: "none" }` with no reason
            // ("acceptance.reason is required when level is none."), so this is belt-and-braces —
            // and for every level above `none` the two constructors are identical anyway.
            let can_disable = config
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| !reason.trim().is_empty());
            let policy = lower_acceptance_policy(config);
            match level.and_then(level_to_status) {
                Some(status) if can_disable => {
                    Ok(Some(policy.apply(AcceptanceContract::explicit(status, verify))))
                }
                Some(status) => Ok(Some(
                    policy.apply(AcceptanceContract::explicit_floor(status, verify)),
                )),
                // `{ verify: [...] }` with no `level` is pi's `level: "auto"` default
                // (`acceptance.ts:149-154` normalizes an absent level to `auto`), so the level is
                // still inferred — but declared `verify[]` commands must not be dropped, so an
                // object carrying any is lowered as an explicit `verified` contract.
                None if !verify.is_empty() => Ok(Some(policy.apply(AcceptanceContract::explicit(
                    AcceptanceStatus::Verified,
                    verify,
                )))),
                // `{ criteria: [...] }` / `{ evidence: [...] }` with no `level` and no `verify[]` is
                // ALSO pi's `auto`: the level is inferred, but `resolveEffectiveAcceptance` still
                // resolves the declared criteria/evidence/review/stopRules and
                // `formatAcceptancePrompt`/`evaluateAcceptance` still consume them
                // (`acceptance.ts:344-401`). Lowering it as a `none` FLOOR reproduces that — the
                // floor is discarded by [`AcceptanceContract::resolve_effective`]'s max in favour of
                // the inferred level, and the policy rides along. Returning `None` here (as this arm
                // did before SUBA-C13) threw the whole policy away.
                None if policy.is_declared() => Ok(Some(
                    policy.apply(AcceptanceContract::explicit_floor(
                        AcceptanceStatus::NotRequired,
                        Vec::new(),
                    )),
                )),
                None => Ok(None),
            }
        }
        // `null`/absent is pi's `undefined`.
        _ => Ok(None),
    }
}

/// Lower one authored `acceptance.verify[i]` object onto a [`VerifyCommand`], carrying **every**
/// key upstream's `ACCEPTANCE_VERIFY_KEYS` admits (`acceptance.ts:52` @v0.43.0) —
/// `id`/`command`/`timeoutMs`/`cwd`/`env`/`allowFailure`. Before SUBA-C12b only `command`
/// survived, so a user who authored `{ id: "lint", command: "npm run lint", allowFailure: true }`
/// passed validation and then had `allowFailure` silently dropped, rejecting the run.
///
/// `command` is the only required-at-lowering key: an entry without it is skipped rather than
/// lowered to an empty shell command. That is unreachable in practice —
/// [`model::validate_acceptance_input`] runs first and already rejects a missing/blank `command`
/// (`acceptance.ts:210`) — but keeping the filter makes this function total on arbitrary JSON.
///
/// `id` falls back to `verify[{index}]` for the same defensive reason (upstream requires it,
/// `acceptance.ts:209`); it is used only for diagnostics, never for dispatch.
fn lower_verify_command(item: &serde_json::Value, index: usize) -> Option<VerifyCommand> {
    let command = item.get("command").and_then(serde_json::Value::as_str)?;
    Some(VerifyCommand {
        id: item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| format!("verify[{index}]"), str::to_string),
        command: command.to_string(),
        timeout_ms: item.get("timeoutMs").and_then(serde_json::Value::as_u64),
        cwd: item
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        env: item
            .get("env")
            .and_then(serde_json::Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(key, value)| {
                        value
                            .as_str()
                            .map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            }),
        allow_failure: item.get("allowFailure").and_then(serde_json::Value::as_bool),
    })
}

/// The `criteria`/`evidence`/`review`/`stopRules` half of an authored `acceptance` object, resolved
/// exactly as pi's `resolveEffectiveAcceptance` resolves them (`acceptance.ts:384-395` @v0.43.0):
/// evidence de-duplicated in declaration order, criteria normalized AGAINST that evidence so a
/// criterion declaring none inherits the config-level list.
struct LoweredAcceptancePolicy {
    criteria: Vec<model::ResolvedAcceptanceGate>,
    evidence: Vec<model::AcceptanceEvidenceKind>,
    review: Option<model::ReviewSetting>,
    stop_rules: Vec<String>,
}

impl LoweredAcceptancePolicy {
    /// Whether the authored object declared ANY of the four — the test the no-`level`, no-`verify[]`
    /// arm of [`lower_acceptance_input`] uses to decide between "a real policy at the inferred
    /// level" and pi's own `undefined`.
    fn is_declared(&self) -> bool {
        !self.criteria.is_empty()
            || !self.evidence.is_empty()
            || self.review.is_some()
            || !self.stop_rules.is_empty()
    }

    fn apply(self, contract: AcceptanceContract) -> AcceptanceContract {
        contract.with_policy(self.criteria, self.evidence, self.review, self.stop_rules)
    }
}

/// Lower the four policy keys of an authored `acceptance` object.
///
/// Every arm is deliberately total on arbitrary JSON — [`model::validate_acceptance_input`] has
/// already run by the time [`lower_acceptance_input`] calls this and rejected every malformed
/// shape with pi's own message, so a value that does not parse here is unreachable in practice and
/// is skipped rather than defaulted to something a policy author did not write.
fn lower_acceptance_policy(
    config: &serde_json::Map<String, serde_json::Value>,
) -> LoweredAcceptancePolicy {
    // `evidence: AcceptanceEvidenceKind[]` (shared/types.ts:285), de-duplicated by
    // `[...new Set(...)]` (acceptance.ts:283-285).
    let evidence = model::unique_evidence(
        &config
            .get("evidence")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(model::AcceptanceEvidenceKind::from_wire)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );

    // `criteria: Array<string | AcceptanceGate>` (shared/types.ts:284).
    let criteria_input: Vec<model::CriterionInput> = config
        .get("criteria")
        .and_then(serde_json::Value::as_array)
        .map(|items| items.iter().filter_map(lower_criterion).collect())
        .unwrap_or_default();
    // `normalizeCriteria(criteria, evidence)` (acceptance.ts:296) — the evidence list is the
    // SECOND argument, i.e. a gate that declares no `evidence` of its own inherits the config's.
    let criteria = model::normalize_criteria(&criteria_input, &evidence);

    // `review: AcceptanceReviewGate | false` (shared/types.ts:288).
    let review = match config.get("review") {
        Some(serde_json::Value::Bool(flag)) => Some(model::ReviewSetting::Disabled(*flag)),
        Some(serde_json::Value::Object(gate)) => {
            Some(model::ReviewSetting::Gate(model::AcceptanceReviewGate {
                agent: gate
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                focus: gate
                    .get("focus")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                required: gate.get("required").and_then(serde_json::Value::as_bool),
            }))
        }
        _ => None,
    };

    // `stopRules: string[]` (shared/types.ts:289).
    let stop_rules = config
        .get("stopRules")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    LoweredAcceptancePolicy {
        criteria,
        evidence,
        review,
        stop_rules,
    }
}

/// Lower one authored `acceptance.criteria[i]` — a bare `must` string, or a full `AcceptanceGate`
/// object (shared/types.ts:261-266). Anything else yields `None` (unreachable past validation).
fn lower_criterion(item: &serde_json::Value) -> Option<model::CriterionInput> {
    match item {
        serde_json::Value::String(must) => Some(model::CriterionInput::Text(must.clone())),
        serde_json::Value::Object(gate) => {
            Some(model::CriterionInput::Gate(model::AcceptanceGate {
                id: gate
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                must: gate
                    .get("must")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                evidence: gate.get("evidence").and_then(serde_json::Value::as_array).map(
                    |items| {
                        items
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .filter_map(model::AcceptanceEvidenceKind::from_wire)
                            .collect()
                    },
                ),
                severity: match gate.get("severity").and_then(serde_json::Value::as_str) {
                    Some("recommended") => Some(model::GateSeverity::Recommended),
                    Some("required") => Some(model::GateSeverity::Required),
                    _ => None,
                },
            }))
        }
        _ => None,
    }
}

/// The exact heading this module injects and later scans for — kept as a named constant so
/// [`model::format_acceptance_prompt`] (which emits it) and any future re-detection logic never
/// drift out of sync with each other over a hand-typed literal.
const ACCEPTANCE_CONTRACT_HEADING: &str = "## Acceptance Contract";

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
/// produced by [`model::format_acceptance_prompt`] over a [`model::ResolvedAcceptanceConfig`]
/// projected from `contract` by [`AcceptanceContract::to_resolved_config`]: the acceptance level,
/// the `- <id>: <must>` criteria list, the `Required evidence: <kinds>` line, the parent's
/// `verify[]` commands, the review gate, the stop rules, and the exact `acceptance-report` JSON
/// template. That template names `criteriaSatisfied` plus every one of
/// `exec/output.rs::ACCEPTANCE_REPORT_COMPANION_KEYS`, so a compliant child's block is reliably
/// picked up by both R-SA-029's final-output extraction AND this module's own
/// [`extract_acceptance_report`].
///
/// Before SUBA-C13 this function wrote its own fixed block that mentioned only the required level,
/// the report keys and the `verify[]` list — a child was never told the criteria it was about to be
/// gated on, which meant it had no reason to report them and
/// [`model::check_criteria_satisfied`] would have failed every one of them.
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
    let block = model::format_acceptance_prompt(&contract.to_resolved_config(), false);
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

impl AcceptanceStatus {
    /// The wire/prose string for this level, matching [`AcceptanceStatus`]'s own `serde`
    /// `kebab-case` rendering (`"not-required"`, `"claimed"`, …) — used by
    /// [`inject_acceptance_contract`] so the text shown to the child matches exactly what this
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

// ============================================================================================
// R-SA-032 / DI-SA-5: verify[] REAL subprocess execution
// ============================================================================================

/// R-SA-032 / DI-SA-5 (MUST) — actually execute every command in `commands`, IN ORDER, as a real
/// OS subprocess each, observing each one's real exit code. Returns one
/// [`model::AcceptanceVerifyResult`] per command, always in the same order as `commands` — this
/// function does NOT short-circuit on the first failure (a caller wants to see every command's real
/// outcome for a rejected run's detail text, not just the first one that failed), but callers
/// deciding overall pass/fail MUST require that no result
/// [`rejects`](model::AcceptanceVerifyResult::rejects) (see [`evaluate_acceptance`]) — NOT that
/// every result exited 0, since a declared `allowFailure: true` command that exits nonzero is
/// [`model::VerifyRunStatus::AllowedFailure`] and still must not reject the run
/// (`acceptance.ts:1193,1297`).
///
/// # This is a THIN LOOP over the one runner, not a second one
///
/// Upstream has exactly ONE verify runner (`runMemoizedVerifyCommand` -> `runVerifyCommand`,
/// `pi-subagents/src/runs/shared/acceptance.ts:1072-1208` @v0.43.0) producing exactly ONE result
/// type (`AcceptanceVerifyResult`, `shared/types.ts:736-758`), and BOTH of its live gates
/// (`runs/foreground/execution.ts:1696-1706`, `runs/background/subagent-runner.ts:1628-1640`) go
/// through it. This crate used to carry a SECOND transcription of that runner here, writing a
/// second result type (`VerifyCommandResult`) that had no field for any of upstream's memoization
/// evidence — so `artifactPath`/`cacheKey`/`memoized`/`envKeys`/`envHash`/`workspaceState` were
/// dropped outright on the live foreground path and `artifactError` was downgraded to a
/// `tracing::debug!`. Both paths now go through [`model::run_memoized_verify_command`]; this
/// function is the sequential loop upstream writes inline at `acceptance.ts:1288-1296`.
///
/// `default_cwd` is the run-level working directory: used verbatim for a command declaring no
/// `cwd`, and as the base a relative declared `cwd` resolves against (`acceptance.ts:1078`).
pub async fn run_verify_commands(
    commands: &[VerifyCommand],
    default_cwd: &Path,
) -> Vec<model::AcceptanceVerifyResult> {
    run_verify_commands_memoized(commands, default_cwd, None).await
}

/// G80 — [`run_verify_commands`] with upstream's per-workspace MEMOIZATION armed
/// (`runMemoizedVerifyCommand`, `pi-subagents/src/runs/shared/acceptance.ts:1072-1132` @v0.43.0).
///
/// This is the live foreground gate's entry point: pi calls `evaluateAcceptance({ …, artifactsDir,
/// runId })` for a single run (`runs/foreground/execution.ts:1704-1705`) and for every background
/// step (`runs/background/subagent-runner.ts:1638-1639`), and those are the two call sites whose
/// verify results are memoized. `memo: None` reproduces the un-memoized behavior exactly — no
/// artifact is read, none is written, every command executes, and none of the seven evidence fields
/// is stamped — which is also what pi's chain group gate does (`chain-execution.ts:1037-1046`
/// passes neither field).
///
/// A memo HIT replays the recorded `exit_code`/`status`/`stdout`/`stderr`/`duration_ms` without
/// spawning anything; the cache is keyed on the command's text, its resolved repo-relative cwd, its
/// declared env key names, a hash of the whole effective environment, its timeout, its
/// `allow_failure` flag, the repository `HEAD` and a hash of the full working-tree diff
/// (`acceptance.ts:1091-1101`). Any edit anywhere in the tree therefore invalidates every memo,
/// which is what makes replaying a `cargo test` result safe.
pub async fn run_verify_commands_memoized(
    commands: &[VerifyCommand],
    default_cwd: &Path,
    memo: Option<model::VerifyMemoContext<'_>>,
) -> Vec<model::AcceptanceVerifyResult> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        results.push(model::run_memoized_verify_command(command, default_cwd, memo).await);
    }
    results
}


/// Read one of a child's piped streams to EOF on its own task, so neither stream can fill its
/// kernel pipe buffer and deadlock the `child.wait()` the timeout races against.
fn spawn_pipe_drain<R>(mut pipe: R) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt as _;
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf).await;
        buf
    })
}

/// Collect a [`spawn_pipe_drain`] task's bytes, treating an absent pipe or a join failure as
/// "no output" — output capture is diagnostic detail here, never a reason to fail a command whose
/// real exit code was already observed.
async fn drained(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Collect BOTH [`spawn_pipe_drain`] tasks, but never past `deadline` — returning `None` (and
/// aborting both tasks, releasing this process's read ends) when the deadline passes first.
///
/// This is the single bound that keeps a verify command's own `timeoutMs` honest, used by the one
/// runner ([`model::run_verify_command`]).
///
/// # Why an unbounded collect is a hang, not a slow path
///
/// `spawn_pipe_drain` reads to EOF, and a pipe reaches EOF only when the LAST write end closes —
/// including the copies every descendant inherited. `child.wait()` returns as soon as the DIRECT
/// child exits, so a routine `verify[]` entry like `./server &`, `npm run dev &` or any script that
/// daemonises leaves the write end held for the descendant's whole lifetime. Awaiting the drain
/// tasks after `wait()` with no bound therefore blocks `run_verify_commands` (which loops these
/// sequentially, with no outer timeout) forever, silently — worse than the abandoned-child bug
/// SUBA-027 fixed.
///
/// # Why the deadline is absolute rather than a fresh post-`wait()` grace
///
/// Upstream arms ONE `setTimeout(abortVerification, timeoutMs)` at spawn
/// (`pi-subagents/src/runs/shared/acceptance.ts:759` @v0.34.0) and settles on Node's `"close"`
/// event, which — exactly like `read_to_end` — waits for every stdio stream to close. When a
/// descendant holds them open, upstream's `"close"` never fires and `abortVerification`'s
/// `hardKill` `finish({status: "timed-out", …})` (`:742-758`) is what resolves the promise, 1000 ms
/// after the deadline. So upstream reports such a command as TIMED OUT at `timeoutMs + 1000ms`
/// regardless of the exit code it already observed, and this port does the same rather than
/// inventing a separate, shorter grace with a different verdict.
async fn drained_by(
    deadline: tokio::time::Instant,
    stdout_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let aborts: Vec<tokio::task::AbortHandle> = [stdout_task.as_ref(), stderr_task.as_ref()]
        .into_iter()
        .flatten()
        .map(tokio::task::JoinHandle::abort_handle)
        .collect();
    let collect = async move {
        let out = drained(stdout_task).await;
        let err = drained(stderr_task).await;
        (out, err)
    };
    match tokio::time::timeout_at(deadline, collect).await {
        Ok(pair) => Some(pair),
        Err(_elapsed) => {
            for handle in aborts {
                handle.abort();
            }
            None
        }
    }
}

/// Build the platform shell invocation for one `verify[]` command string.
fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
}

// ============================================================================================
// R-SA-032: acceptance-gate evaluation
// ============================================================================================

/// The subset of a finished, otherwise-clean attempt's state [`evaluate_acceptance`] needs —
/// deliberately narrow (mirrors `exec/fallback.rs::AttemptSignal`'s own "only what the decision
/// actually branches on" convention) rather than depending on the not-yet-built `SingleResult`.
#[derive(Debug, Clone, Copy)]
pub struct CleanCompletionGate {
    pub exit_code: i32,
    pub detached: bool,
    pub interrupted: bool,
    pub timed_out: bool,
}

impl CleanCompletionGate {
    /// R-SA-033's own gate condition, restated as a predicate: `exit_code == 0 && !detached &&
    /// !interrupted && !timed_out`.
    #[must_use]
    pub fn is_clean(self) -> bool {
        self.exit_code == 0 && !self.detached && !self.interrupted && !self.timed_out
    }
}

/// R-SA-032 (MUST) / DI-SA-5 — evaluate the acceptance ledger for one finished attempt.
///
/// # Preconditions (defensively re-checked, never merely trusted to caller discipline)
///
/// `gate` MUST report [`CleanCompletionGate::is_clean`] — this function immediately returns a
/// [`AcceptanceLedger::not_required`]-shaped no-op (NOT an error: skipping evaluation on a
/// non-clean run is itself the correct, expected behavior R-SA-032's own text specifies — "after
/// a clean run... the orchestrator MUST evaluate") if the gate is not clean, regardless of what
/// `contract` demands. This mirrors R-SA-033's own explicit gate condition rather than requiring
/// every call site to remember to check it first.
///
/// # Evaluation ladder
///
/// 1. `contract.is_no_op()` → [`AcceptanceStatus::NotRequired`], no further work.
/// 2. Otherwise, extract the child's own `acceptance-report` block (if any) from `final_output`
///    via [`extract_acceptance_report`] — its PRESENCE (not its claimed truthiness) establishes
///    at least [`AcceptanceStatus::Claimed`]; if the extracted JSON additionally carries at least
///    one of [`crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS`], the floor rises to
///    [`AcceptanceStatus::Attested`] (still self-report, richer self-report).
/// 3. If `contract.required_level >= Checked`, a structural/evidence check runs:
///    `completion_guard.triggered == false` (i.e. the completion-mutation guard did NOT fire) is
///    the evidence this module consults — an orchestrator-observed fact, not a child assertion —
///    and raising the achieved level to [`AcceptanceStatus::Checked`] when it holds.
/// 4. If `contract.required_level >= Verified`, [`run_verify_commands`] is ACTUALLY invoked
///    against every declared command; achieving [`AcceptanceStatus::Verified`] requires that **no**
///    result [`rejects`](model::AcceptanceVerifyResult::rejects) — a single failing (or timed-out)
///    command caps the achieved level below `Verified` regardless of how many others passed, and
///    regardless of anything the child's own report claims. A command that declared
///    `allowFailure: true` and merely exited nonzero is [`model::VerifyRunStatus::AllowedFailure`]
///    and does NOT cap the level — upstream `evaluateAcceptance` rejects only on
///    `status === "failed" || status === "timed-out"` (`acceptance.ts:1336` @v0.43.0).
/// 5. If `contract.required_level >= Reviewed`, `contract.reviewer_result` MUST be
///    `Some(ReviewerResult { approved: true, .. })` to reach [`AcceptanceStatus::Reviewed`]; a
///    `None` reviewer result or an `approved: false` one caps the achieved level below `Reviewed`.
/// 6. If the achieved level (after steps 2-5) does not satisfy `contract.required_level`
///    ([`AcceptanceStatus::satisfies`]), the final status is [`AcceptanceStatus::Rejected`] with a
///    `detail` explaining which required level was not reached; otherwise the achieved level
///    itself is the final status.
///
/// `verify_results` on the returned ledger is populated iff step 4 actually ran (i.e.
/// `contract.required_level >= Verified`); empty otherwise.
///
/// # Relationship to [`model::evaluate_acceptance`]
///
/// Upstream has ONE gate. This is the one `exec/mod.rs::run_sync` calls for every ordinary run;
/// [`model::evaluate_acceptance`] is the faithful port of the same upstream function, and its only
/// production caller is `spawn::chain_graph`'s dynamic-group gate. That split is unfinished port
/// work — see the header comment above [`model`] for exactly which layers have already collapsed
/// onto the [`model`] side and which have not.
///
/// What matters for a reader of the RETURNED ledger: every piece of evidence upstream records on a
/// verify run reaches this ledger, because [`AcceptanceLedger::verify_results`] holds upstream's
/// own [`model::AcceptanceVerifyResult`], and [`AcceptanceLedger::evidence_status`] is upstream's
/// own `evidenceStatus`. What does NOT reach it is upstream's structured `runtimeChecks` /
/// `childReport` / `reviewResult`: this function still flattens its findings into `detail`.
#[must_use]
pub async fn evaluate_acceptance(
    contract: &AcceptanceContract,
    gate: CleanCompletionGate,
    final_output: Option<&str>,
    completion_guard: CompletionMutationGuardResult,
    verify_cwd: &Path,
    // G80 — pi's `artifactsDir`/`runId` pair (`acceptance.ts:1226-1227`), threaded down to
    // `runMemoizedVerifyCommand`. `None` (every caller with no artifacts root configured, and
    // every test that does not exercise memoization) executes every verify[] command for real.
    memo: Option<model::VerifyMemoContext<'_>>,
    // G82 — pi's `fileOutput` (`acceptance.ts:1214-1220`): content the CHILD itself sent to its
    // configured output file, taken from its own successful `write` tool calls rather than from
    // disk, so a concurrent writer to the same path cannot be misattributed to it. `None` for a
    // run with no configured output path, or one whose child never successfully wrote it.
    file_output: Option<AcceptanceFileOutput<'_>>,
) -> AcceptanceLedger {
    if !gate.is_clean() {
        return AcceptanceLedger::not_required();
    }
    if contract.is_no_op() {
        return AcceptanceLedger::not_required();
    }

    // G82 / pi `parseAcceptanceReportSources` (`acceptance.ts:753-771`): the acceptance report may
    // live in the assistant's own output OR in the artifact the child wrote to its configured
    // output path, and in `outputMode: "file-only"` the FILE is searched first. Resolved once here
    // and used by every rung below that reads the report.
    let report_source = select_acceptance_report_source(final_output, file_output.as_ref());

    // Step 2: self-report floor (Claimed / Attested).
    let mut achieved = self_report_floor(report_source);

    // Step 3: Checked — orchestrator-observed structural evidence.
    let mut detail: Vec<String> = Vec::new();
    if contract.required_level >= AcceptanceStatus::Checked {
        let mut checked = true;
        if completion_guard.triggered {
            detail.push(
                "checked: completion-mutation guard triggered (implementation was expected but \
                 no mutating tool call was observed)"
                    .to_string(),
            );
            checked = false;
        }
        // pi's own `rank >= checked` rung (`evaluateAcceptance`, `acceptance.ts:1297-1321`): every
        // declared criterion must appear in the child's report as `satisfied`, every declared
        // evidence kind must be present in it, and `no-staged-files` additionally shells out to a
        // REAL `git status --short`. ANY failed runtime check rejects. Runs alongside — not instead
        // of — the completion-mutation guard above, which is this crate's own extra orchestrator-
        // observed signal (R-SA-034) and has no upstream counterpart on this rung.
        let failures = declared_structural_failures(contract, report_source, verify_cwd).await;
        if !failures.is_empty() {
            detail.extend(failures);
            checked = false;
        }
        if checked {
            achieved = achieved.max(AcceptanceStatus::Checked);
        }
    }

    // Step 4: Verified — REAL verify[] execution (DI-SA-5's whole reason for being).
    let mut verify_results = Vec::new();
    if contract.required_level >= AcceptanceStatus::Verified {
        if contract.verify.is_empty() {
            detail.push("verified: no verify[] commands were declared".to_string());
        } else {
            verify_results =
                run_verify_commands_memoized(&contract.verify, verify_cwd, memo).await;
            // `verifyRuns.some((run) => run.status === "failed" || run.status === "timed-out")`
            // (`acceptance.ts:1336` @v0.43.0) — NOT `!every(passed)`, which would also reject a
            // command the author explicitly marked `allowFailure: true`.
            let failed: Vec<&str> = verify_results
                .iter()
                .filter(|r| r.rejects())
                .map(|r| r.command.as_str())
                .collect();
            if failed.is_empty() {
                achieved = achieved.max(AcceptanceStatus::Verified);
            } else {
                detail.push(format!(
                    "verified: {} of {} verify[] command(s) failed: {}",
                    failed.len(),
                    verify_results.len(),
                    failed.join(", ")
                ));
            }
        }
    }

    // G78 — upstream's `evidenceStatus` is frozen HERE, before the review rung: `evaluateAcceptance`
    // writes both fields in lockstep through the attestation/checked/verified rungs
    // (`acceptance.ts:1254-1255,1275-1276,1303-1304`) and its review block (`:1318-1336`) then
    // rewrites `status` ALONE. Snapshotting `achieved` at this exact point is that same freeze:
    // everything above is evidence the orchestrator observed, everything below is review.
    let evidence_achieved = achieved;

    // Step 5: Reviewed — externally-supplied reviewer result only.
    if contract.required_level >= AcceptanceStatus::Reviewed {
        match &contract.reviewer_result {
            Some(ReviewerResult { approved: true, .. }) => {
                achieved = achieved.max(AcceptanceStatus::Reviewed);
            }
            Some(ReviewerResult {
                approved: false,
                detail: reviewer_detail,
            }) => {
                detail.push(format!(
                    "reviewed: independent reviewer did not approve{}",
                    reviewer_detail
                        .as_deref()
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default()
                ));
            }
            // pi `evaluateAcceptance`'s `optionalReview` branch (`acceptance.ts:1321-1335`): a
            // declared `review: { required: false }` gate makes a missing reviewer result a
            // NON-BLOCKING finding rather than a rejection, so the run keeps whatever level it
            // otherwise reached. Any other shape (a required gate, `review: false`, or no `review`
            // key at all) still caps the level below `Reviewed`.
            None if matches!(
                &contract.review,
                Some(model::ReviewSetting::Gate(gate)) if gate.required == Some(false)
            ) =>
            {
                achieved = achieved.max(AcceptanceStatus::Reviewed);
            }
            None => {
                detail.push(
                    "reviewed: no independent reviewer result was supplied".to_string(),
                );
            }
        }
    }

    // Step 6: final verdict.
    if achieved.satisfies(contract.required_level) {
        AcceptanceLedger {
            status: achieved,
            evidence_status: evidence_achieved.evidence_status(),
            detail: if detail.is_empty() {
                None
            } else {
                Some(detail.join("; "))
            },
            verify_results,
        }
    } else {
        let reason = if detail.is_empty() {
            format!(
                "required {} but only reached {}",
                contract.required_level.as_wire_str(),
                achieved.as_wire_str()
            )
        } else {
            detail.join("; ")
        };
        AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            // Every rejecting arm upstream writes `rejected` to BOTH fields
            // (`acceptance.ts:1260-1261,1283-1284,1298-1299,1309-1310`) — a rejection is not an
            // evidence level that was earned and then withheld pending review, so unlike the review
            // rung it does move `evidenceStatus`.
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: Some(reason),
            verify_results,
        }
    }
}

/// Step 3's declared-policy half: pi's `checkCriteriaSatisfied` + `runStructuralChecks`
/// (`acceptance.ts:911-966` @v0.43.0, applied on `evaluateAcceptance`'s `rank >= checked` rung at
/// `:819-829`), run against the child's own `acceptance-report` block.
///
/// Returns one message per FAILED [`model::AcceptanceRuntimeCheck`], verbatim, so the ledger detail
/// (and therefore [`apply_post_hoc_correction`]'s error text) carries pi's own wording —
/// e.g. `tests-added evidence missing from child report.` or
/// `Required criterion 'c1' was not reported.`. An empty vector means every declared check passed,
/// which is also the trivial answer when the contract declares no criteria and no evidence.
///
/// The child's report is parsed by [`model::parse_acceptance_report`] — the same faithful port
/// `exec/mod.rs` already uses to STRIP the block from the delivered output — rather than by this
/// module's own [`extract_acceptance_report`], which is a presence/companion-key probe for the
/// `Claimed` vs `Attested` floor and cannot answer "is criterion `c1` reported satisfied". A child
/// that emitted no parseable report at all is treated as an empty report, so every declared check
/// fails with pi's "missing"/"not reported" message rather than being skipped.
async fn declared_structural_failures(
    contract: &AcceptanceContract,
    final_output: Option<&str>,
    cwd: &Path,
) -> Vec<String> {
    if contract.criteria.is_empty() && contract.evidence.is_empty() {
        return Vec::new();
    }
    let report = final_output
        .map(model::parse_acceptance_report)
        .and_then(|parsed| parsed.report)
        .unwrap_or_default();
    let mut checks = model::check_criteria_satisfied(&contract.criteria, &report);
    checks.extend(model::run_structural_checks(&contract.evidence, &report, cwd).await);
    checks
        .into_iter()
        .filter(|check| check.status == model::RuntimeCheckStatus::Failed)
        .map(|check| check.message)
        .collect()
}

// ============================================================================================
// G82: the child-authored output file as an acceptance-report source
// (pi `parseAcceptanceReportSources`, `acceptance.ts:753-771`)
// ============================================================================================

/// G82 — pi's `fileOutput` input to `evaluateAcceptance` (`acceptance.ts:1214-1220`). Upstream's
/// own doc on the field:
///
/// > Content the child sent to its configured output file (from its own write tool calls, not from
/// > disk, so a concurrent writer to the same path cannot be misattributed). Searched for the
/// > acceptance report; searched before the assistant output when `authoritative` (outputMode
/// > "file-only").
///
/// Built by [`crate::exec::output::extract_child_written_output`], never by reading the path.
#[derive(Debug, Clone, Copy)]
pub struct AcceptanceFileOutput<'a> {
    /// The exact bytes the child's own successful `write` call sent to `path`.
    pub content: &'a str,
    /// The configured output path, quoted verbatim in the "(in configured output …)" parse-error
    /// suffix upstream produces (`acceptance.ts:763`).
    pub path: &'a Path,
    /// `outputMode === "file-only"` — the file becomes the PRIMARY report source, searched before
    /// the assistant output.
    pub authoritative: bool,
}

/// G82 — source: `parseAcceptanceReportSources(output, fileOutput)` (`acceptance.ts:753-771`).
///
/// ```text
/// const [primary, secondary] = fileOutput?.authoritative ? [fromFile, fromText] : [fromText, fromFile];
/// const first = primary();
/// // A malformed report in the primary source is a defect to surface, not a
/// // miss to paper over with the secondary source; only a genuinely absent
/// // report falls through.
/// if (first.report || first.error !== ACCEPTANCE_REPORT_NOT_FOUND) return first;
/// return secondary();
/// ```
///
/// Ported as a choice of SOURCE TEXT rather than of parse result, because every rung of
/// [`evaluate_acceptance`] re-reads the report from text ([`self_report_floor`]'s companion-key
/// probe and [`declared_structural_failures`]'s full parse want the same source). The selection
/// rule is identical: the primary source wins whenever it yields a report OR any parse error other
/// than "not found"; only a genuinely absent report falls through to the secondary.
fn select_acceptance_report_source<'a>(
    output: Option<&'a str>,
    file_output: Option<&AcceptanceFileOutput<'a>>,
) -> Option<&'a str> {
    /// `ACCEPTANCE_REPORT_NOT_FOUND` (`acceptance.ts:699`) — the one error that is a MISS rather
    /// than a defect. Reuses the model port's own constant so the two selectors can never drift.
    use model::ACCEPTANCE_REPORT_NOT_FOUND;

    let from_text = output;
    let from_file = file_output.map(|f| f.content);
    let (primary, secondary) = if file_output.is_some_and(|f| f.authoritative) {
        (from_file, from_text)
    } else {
        (from_text, from_file)
    };
    let primary_is_decisive = primary.is_some_and(|text| {
        let parsed = model::parse_acceptance_report(text);
        parsed.report.is_some() || parsed.error.as_deref() != Some(ACCEPTANCE_REPORT_NOT_FOUND)
    });
    if primary_is_decisive {
        primary
    } else {
        // `return secondary()` — and when there is no secondary at all, its result is the same
        // "not found" the primary already produced, so the primary text is kept for the (identical)
        // downstream outcome rather than discarding the run's own output.
        secondary.or(primary)
    }
}

/// Step 2's self-report floor: `NotRequired` if no `final_output` or no acceptance-report-shaped
/// block is present at all; `Claimed` if a block is present but carries no recognizable companion
/// evidence field; `Attested` if it carries at least one of
/// [`crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS`] alongside `criteriaSatisfied`.
fn self_report_floor(final_output: Option<&str>) -> AcceptanceStatus {
    let Some(text) = final_output else {
        return AcceptanceStatus::NotRequired;
    };
    if !looks_like_acceptance_report(text) {
        return AcceptanceStatus::NotRequired;
    }
    if extract_acceptance_report(text).is_some_and(|report| report.has_companion_evidence) {
        AcceptanceStatus::Attested
    } else {
        AcceptanceStatus::Claimed
    }
}

/// A minimally-parsed view of one child's self-reported `acceptance-report` block, used only to
/// decide the `Claimed` vs. `Attested` self-report floor (step 2 above) — this is NOT a full
/// schema validator (that is R-SA-030's `exec/output.rs`/structured-output concern for a
/// DIFFERENT, schema-declared structured output, not this prose-embedded report block).
#[derive(Debug, Clone)]
struct ParsedAcceptanceReport {
    has_companion_evidence: bool,
}

/// Extract and minimally interpret the acceptance-report-shaped block from `text` (the same shape
/// [`crate::exec::output::looks_like_acceptance_report`] detects), if any. Reuses that function's
/// own detection logic rather than re-implementing fenced-block scanning a second time — this
/// function's only additional job is deciding whether the block, once located, carries a
/// companion-evidence key.
fn extract_acceptance_report(text: &str) -> Option<ParsedAcceptanceReport> {
    if !looks_like_acceptance_report(text) {
        return None;
    }
    // G79 gave every acceptance-report field a snake_case alias (`acceptance.ts:486-508`), so the
    // companion-evidence probe has to accept both spellings or a child whose report the PARSER
    // accepts in full is still scored as a bare `Claimed` here.
    let has_companion_evidence = crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS
        .iter()
        .any(|key| crate::exec::output::text_mentions_report_key(text, key));
    Some(ParsedAcceptanceReport {
        has_companion_evidence,
    })
}

// ============================================================================================
// R-SA-033: post-hoc exit-code correction, strictly ordered
// ============================================================================================

/// The result of applying [`apply_post_hoc_correction`] — either the run's exit code/error are
/// left untouched, or both are rewritten per R-SA-033.
#[derive(Debug, Clone, PartialEq)]
pub struct PostHocCorrection {
    /// The exit code to report — unchanged from the input unless a correction was applied.
    pub exit_code: i32,
    /// The error text to report — unchanged from the input unless a correction was applied, in
    /// which case the rejection reason is appended to whatever error text (if any) was already
    /// present.
    pub error: Option<String>,
    /// Whether a correction was actually applied — exposed so a caller can log/assert on it
    /// separately from just re-deriving it by comparing `exit_code`s (which would be ambiguous
    /// if the input `exit_code` also happened to already be nonzero for an unrelated reason —
    /// though per R-SA-033's own gate that case never reaches this function to begin with, since
    /// [`evaluate_acceptance`] itself already required `gate.is_clean()`, i.e. `exit_code == 0`,
    /// before ever producing a non-`NotRequired` ledger).
    pub corrected: bool,
}

/// A nonzero sentinel exit code this module uses when correcting an acceptance rejection
/// (R-SA-033: "overwrite the reported `exit_code` to a nonzero value"). `1` is the conventional
/// generic-failure exit code and carries no further encoded meaning — the actual reason always
/// lives in the accompanying error text, never in this numeric value alone.
pub const ACCEPTANCE_REJECTED_EXIT_CODE: i32 = 1;

/// R-SA-033 (MUST) — post-hoc exit-code correction, STRICTLY ORDERED to run after structured-
/// output validation (R-SA-030) and the completion-mutation guard (R-SA-034), and strictly before
/// the result is handed to the caller or serialized. This function itself does not enforce that
/// ordering procedurally (it cannot — it has no visibility into when its caller invokes it
/// relative to those other two checks); it is the caller's ([`crate::exec::mod`]'s not-yet-built
/// `run_sync` completion path, a later phase) responsibility to invoke this LAST, after
/// [`evaluate_acceptance`] itself, which is exactly why this function takes an already-computed
/// [`AcceptanceLedger`] as input rather than re-deriving one.
///
/// Correction is applied if AND ONLY IF all three hold:
/// 1. `ledger.status == AcceptanceStatus::Rejected` (the gate rejected the run);
/// 2. `contract_was_explicit` (R-SA-033: "AND the acceptance contract was explicit AND..." — a
///    heuristically-inferred contract's rejection is recorded on the ledger but never flips an
///    otherwise-successful `exit_code`, matching [`AcceptanceContract::explicit`]'s own doc
///    comment);
/// 3. the run otherwise succeeded per `gate.is_clean()` (restated here defensively rather than
///    trusted to the caller, exactly as [`evaluate_acceptance`] itself does — a correction applied
///    to an already-nonzero exit code would violate this function's own "a run that already
///    failed the completion guard is not corrected a second time" invariant, arch-SA §6.3.4).
///
/// When applied, `exit_code` becomes [`ACCEPTANCE_REJECTED_EXIT_CODE`] and the rejection reason
/// (`ledger.detail`, falling back to a generic message if absent) is appended to `existing_error`.
#[must_use]
pub fn apply_post_hoc_correction(
    ledger: &AcceptanceLedger,
    contract_was_explicit: bool,
    gate: CleanCompletionGate,
    existing_error: Option<&str>,
) -> PostHocCorrection {
    let should_correct = ledger.status == AcceptanceStatus::Rejected
        && contract_was_explicit
        && gate.is_clean();

    if !should_correct {
        return PostHocCorrection {
            exit_code: gate.exit_code,
            error: existing_error.map(str::to_string),
            corrected: false,
        };
    }

    let reason = ledger
        .detail
        .clone()
        .unwrap_or_else(|| "acceptance criteria were not met".to_string());
    let error = match existing_error {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{existing}; acceptance rejected: {reason}")
        }
        _ => format!("acceptance rejected: {reason}"),
    };

    PostHocCorrection {
        exit_code: ACCEPTANCE_REJECTED_EXIT_CODE,
        error: Some(error),
        corrected: true,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::time::Duration;

    use super::*;

    /// The ONE verify runner (`model::run_verify_command`, upstream `runVerifyCommand`) under the
    /// short local name the single-command tests below used when this module carried a second copy
    /// of it. There is no second copy any more; only the alias survives.
    use model::run_verify_command as run_one_verify_command;

    /// A `verify[]` entry declaring nothing but its shell command — the run-level `cwd`, the
    /// inherited environment and [`model::DEFAULT_VERIFY_TIMEOUT_MS`] all apply.
    fn vc(command: &str) -> VerifyCommand {
        VerifyCommand::shell(command)
    }

    /// The raw exit observation the retired `VerifyCommandResult.passed` field carried:
    /// `exit_code == Some(0)`, verbatim (that field's own doc: "Whether this command counts as
    /// passed: `exit_code == Some(0)`"). Every assertion below therefore keeps exactly the strength
    /// it had before the two verify-result shapes collapsed onto upstream's single
    /// [`model::AcceptanceVerifyResult`]. NOTE it is deliberately NOT `status == Passed`: a command
    /// declaring `allowFailure: true` that exits nonzero was `passed: false` with status
    /// `allowed-failure`.
    fn passed(result: &model::AcceptanceVerifyResult) -> bool {
        result.exit_code == Some(0)
    }

    /// The combined capture the retired `VerifyCommandResult.output_tail` held, reassembled from
    /// upstream's separate `stdout`/`stderr` in the same order the old field concatenated them.
    fn output_tail(result: &model::AcceptanceVerifyResult) -> String {
        format!(
            "{}{}",
            result.stdout.as_deref().unwrap_or_default(),
            result.stderr.as_deref().unwrap_or_default()
        )
    }

    /// A `verify[]` entry declaring its own `timeoutMs`, for the timeout/kill-ladder tests that
    /// must not wait out [`model::DEFAULT_VERIFY_TIMEOUT_MS`].
    fn vc_timeout(command: &str, timeout: Duration) -> VerifyCommand {
        VerifyCommand {
            timeout_ms: Some(
                u64::try_from(timeout.as_millis()).expect("a test timeout fits in u64 ms"),
            ),
            ..VerifyCommand::shell(command)
        }
    }

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
                model::AcceptanceEvidenceKind::ReviewFindings,
                model::AcceptanceEvidenceKind::ResidualRisks,
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
                model::AcceptanceEvidenceKind::ReviewFindings,
                model::AcceptanceEvidenceKind::ResidualRisks,
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
                model::AcceptanceEvidenceKind::ManualNotes,
                model::AcceptanceEvidenceKind::ResidualRisks,
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

    /// `explicitAcceptanceCanDisable` (`acceptance.ts:167-169`) requires a non-blank `reason`, and
    /// a bare `"none"` string carries none. v0.34.0 accepted the string and fell through to
    /// `LEVEL_RANK["none"] >= LEVEL_RANK[inferred]`, leaving the gate armed at the inferred level;
    /// v0.43.0 REFUSES it outright at validation (`acceptance.ts:183`) and says how to write it
    /// properly. Either way the one thing a reasonless `"none"` must never do is disarm the gate —
    /// which is exactly what cyrup did before this pair of fixes — so both halves are asserted:
    /// the string is rejected, and the object form that IS accepted still needs its reason.
    #[test]
    fn a_bare_none_string_cannot_disable_the_gate() {
        let err = lower_acceptance_input(&serde_json::json!("none"))
            .expect_err("a reasonless bare 'none' is rejected at v0.43.0");
        assert_eq!(
            err,
            "acceptance level \"none\" requires a reason; use { level: \"none\", reason: \"...\" }."
        );

        // The object form with no reason is likewise refused, so there is no spelling of a
        // reasonless `none` that reaches the contract at all.
        let object_err = lower_acceptance_input(&serde_json::json!({"level": "none"}))
            .expect_err("`{ level: \"none\" }` with no reason is rejected");
        assert!(
            object_err.contains("acceptance.reason is required when level is none."),
            "{object_err}"
        );

        // And a run with no acceptance param at all still gets the inferred gate, unchanged.
        let effective = AcceptanceContract::resolve_effective(None, "worker", "Implement the fix");
        assert_eq!(effective.required_level, AcceptanceStatus::Checked);
        assert!(!effective.is_no_op(), "the gate must still be evaluated");
    }

    /// Both forms upstream DOES accept as an "off" switch: the `false` shorthand (whose reason
    /// `normalizeAcceptanceInput` supplies itself) and an object `{ level: "none", reason }` with
    /// a non-blank reason.
    #[test]
    fn the_two_disabling_forms_still_turn_the_gate_off() {
        for policy in [
            serde_json::json!(false),
            serde_json::json!({ "level": "none", "reason": "prototype spike, no gate wanted" }),
        ] {
            let lowered = lower_acceptance_input(&policy)
                .expect("a valid policy")
                .expect("a contract");
            assert!(lowered.disables_gate, "must be able to disable: {policy}");

            let effective =
                AcceptanceContract::resolve_effective(Some(lowered), "worker", "Implement the fix");
            assert_eq!(
                effective.required_level,
                AcceptanceStatus::NotRequired,
                "an explicit disable is never raised by the inferred floor: {policy}"
            );
            assert!(effective.is_no_op(), "policy {policy}");
        }
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
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("cargo test")]);
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
    /// would make `model::evaluate_acceptance` reject the run outright for declaring no `verify[]`
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
        assert_eq!(resolved.level, model::AcceptanceLevel::Checked);
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
            model::AcceptanceLevel::Checked
        );
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
        assert_eq!(result.status, model::VerifyRunStatus::Passed);
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
        assert_eq!(result.status, model::VerifyRunStatus::TimedOut);
        // `stderr: trimOutput(redactVerifyEnv(stderr || abortMessage || "Acceptance verification
        // timed out.", …))` (`acceptance.ts:1174`) — the command printed nothing, so upstream's
        // literal fallback is what the ledger carries. Was cyrup's own invented
        // "verify command exceeded its 100ms timeout and was terminated".
        assert_eq!(result.stderr.as_deref(), Some(model::VERIFY_TIMED_OUT_MESSAGE));
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
        assert_eq!(result.status, model::VerifyRunStatus::TimedOut);
        let error = result.stderr.clone().unwrap_or_default();
        assert_eq!(
            error,
            model::VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE,
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
    async fn run_verify_commands_executes_every_command_in_order_and_never_short_circuits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let commands = vec![
            vc("exit 1"), // fails
            vc("exit 0"), // still runs, passes
        ];
        let results = run_verify_commands(&commands, dir.path()).await;
        assert_eq!(results.len(), 2, "both commands must run even though the first failed");
        assert!(!passed(&results[0]));
        assert!(passed(&results[1]));
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-C12b regression: the per-command `verify[]` fields upstream's ACCEPTANCE_VERIFY_KEYS
    // admits (`acceptance.ts:52` @v0.43.0 — id/command/timeoutMs/cwd/env/allowFailure) must
    // actually REACH execution and the gate. `validate_verify_input` has always accepted and
    // type-checked all six, but the contract carried only the command string, so five of them
    // were validated and then silently discarded: a `cwd` ran in the wrong directory, an `env`
    // pair was absent, a `timeoutMs` was replaced by a fixed 300 s, and an `allowFailure: true`
    // command still rejected the run (rewriting its exit code to 1 via
    // `apply_post_hoc_correction`) where upstream reports `allowed-failure` and succeeds.
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lower_acceptance_input_carries_every_declared_verify_field() {
        let contract = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [{
                "id": "lint",
                "command": "npm run lint",
                "timeoutMs": 5000,
                "cwd": "packages/api",
                "env": { "CI": "1" },
                "allowFailure": true,
            }],
        }))
        .expect("a well-formed acceptance policy must lower")
        .expect("an explicit contract");

        assert_eq!(contract.verify.len(), 1);
        let declared = &contract.verify[0];
        assert_eq!(declared.id, "lint");
        assert_eq!(declared.command, "npm run lint");
        assert_eq!(declared.timeout_ms, Some(5000), "timeoutMs must survive lowering");
        assert_eq!(
            declared.cwd.as_deref(),
            Some("packages/api"),
            "cwd must survive lowering"
        );
        assert_eq!(
            declared.env.as_ref().and_then(|env| env.get("CI")).map(String::as_str),
            Some("1"),
            "env must survive lowering"
        );
        assert_eq!(
            declared.allow_failure,
            Some(true),
            "allowFailure must survive lowering"
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

        assert_eq!(result.status, model::VerifyRunStatus::TimedOut);
        assert!(
            elapsed < Duration::from_secs(20),
            "a declared 150ms timeoutMs must bound the command, not DEFAULT_VERIFY_TIMEOUT — took \
             {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allow_failure_maps_a_nonzero_exit_to_allowed_failure_and_does_not_reject() {
        // `status: … passed ? "passed" : command.allowFailure ? "allowed-failure" : "failed"`
        // (`acceptance.ts:1207`) and `evaluateAcceptance`'s reject test, which never names
        // `allowed-failure` (`acceptance.ts:1336`).
        let dir = tempfile::tempdir().expect("tempdir");
        let declared = VerifyCommand {
            allow_failure: Some(true),
            ..VerifyCommand::shell("exit 1")
        };
        let result = run_one_verify_command(&declared, dir.path()).await;

        assert_eq!(result.exit_code, Some(1), "the real exit code is still observed");
        assert!(!passed(&result), "the raw exit observation stays false");
        assert_eq!(result.status, model::VerifyRunStatus::AllowedFailure);
        assert!(!result.rejects(), "an allowed-failure command must not reject the run");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allow_failure_never_rescues_a_timed_out_command() {
        // Upstream tests `timedOut` FIRST in the status ternary (`acceptance.ts:1207`), so a
        // command that hangs is `"timed-out"` — which `evaluateAcceptance` DOES reject — even when
        // it declared `allowFailure: true`.
        let dir = tempfile::tempdir().expect("tempdir");
        let declared = VerifyCommand {
            allow_failure: Some(true),
            timeout_ms: Some(150),
            ..VerifyCommand::shell("sleep 30")
        };
        let result = run_one_verify_command(&declared, dir.path()).await;

        assert_eq!(result.status, model::VerifyRunStatus::TimedOut);
        assert!(result.rejects(), "a timeout rejects regardless of allowFailure");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_allow_failure_command_that_exits_nonzero_still_reaches_verified() {
        // The end-to-end shape from the bug report: an authored
        // `{ level: "verified", verify: [{ id: "lint", command: "…", allowFailure: true }] }`
        // whose command exits nonzero must still be ACCEPTED, so `apply_post_hoc_correction`
        // never rewrites the run's exit code to ACCEPTANCE_REJECTED_EXIT_CODE.
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [
                { "id": "tests", "command": "exit 0" },
                { "id": "lint", "command": "exit 1", "allowFailure": true },
            ],
        }))
        .expect("a well-formed acceptance policy must lower")
        .expect("an explicit contract");

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some("Done.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```"),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Verified,
            "a verify[] command declaring allowFailure: true must not reject the run, got: \
             {ledger:?}"
        );
        assert_eq!(ledger.verify_results.len(), 2);
        assert_eq!(ledger.verify_results[1].status, model::VerifyRunStatus::AllowedFailure);
        assert_eq!(
            apply_post_hoc_correction(&ledger, contract.explicit, clean_gate(), None).exit_code,
            0,
            "an accepted run must not have its exit code rewritten to \
             ACCEPTANCE_REJECTED_EXIT_CODE"
        );
    }

    // ---------------------------------------------------------------------------------------
    // evaluate_acceptance: THE load-bearing DI-SA-5 distinction — a real executed verify[]
    // command that exits 0 reaches Verified; a child merely CLAIMING success in prose does not.
    // ---------------------------------------------------------------------------------------

    fn clean_gate() -> CleanCompletionGate {
        CleanCompletionGate {
            exit_code: 0,
            detached: false,
            interrupted: false,
            timed_out: false,
        }
    }

    fn no_guard_trigger() -> CompletionMutationGuardResult {
        CompletionMutationGuardResult {
            expected_mutation: true,
            attempted_mutation: true,
            triggered: false,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_executed_passing_verify_command_reaches_verified() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract =
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("exit 0")]);

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some("I fixed the bug.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```"),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Verified,
            "a REAL verify[] command that actually exits 0 must reach Verified, got: {ledger:?}"
        );
        assert_eq!(ledger.verify_results.len(), 1);
        assert!(passed(&ledger.verify_results[0]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_childs_prose_claim_of_success_alone_never_reaches_verified() {
        // THE required scenario from the task brief: a child that merely claims success in prose
        // (no verify[] commands declared at all, or verify[] commands declared but the child's
        // own prose asserting they "passed" is irrelevant — this module never even reads that
        // assertion) must NOT reach Verified when Verified is the required level and no REAL
        // verify[] evidence exists.
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![]); // nothing declared to actually run

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some(
                "I ran the tests and they passed.\n```acceptance-report\n\
                 {\"criteriaSatisfied\": true, \"commandsRun\": [\"cargo test\"]}\n```",
            ),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Rejected,
            "a child's own prose claim of success, with NO actually-executed verify[] command, \
             must NOT satisfy a Verified requirement — got: {ledger:?}"
        );
        assert!(ledger.verify_results.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_executed_failing_verify_command_is_rejected_regardless_of_prose_claim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract =
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("exit 1")]);

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some(
                "Everything passed!\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```",
            ),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;

        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert_eq!(ledger.verify_results.len(), 1);
        assert!(!passed(&ledger.verify_results[0]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_failing_command_among_several_caps_below_verified_even_if_others_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(
            AcceptanceStatus::Verified,
            vec![vc("exit 0"), vc("exit 1"), vc("exit 0")],
        );
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path(), None, None)
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert_eq!(ledger.verify_results.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn not_clean_gate_short_circuits_to_not_required_regardless_of_contract() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract =
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![vc("exit 0")]);
        let dirty_gate = CleanCompletionGate {
            exit_code: 1,
            detached: false,
            interrupted: false,
            timed_out: false,
        };
        let ledger =
            evaluate_acceptance(&contract, dirty_gate, None, no_guard_trigger(), dir.path(), None, None).await;
        assert_eq!(ledger.status, AcceptanceStatus::NotRequired);
        assert!(ledger.verify_results.is_empty(), "must not even run verify[] on a non-clean gate");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_level_is_satisfied_by_a_non_triggered_completion_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path(), None, None)
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Checked);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_level_is_rejected_when_completion_guard_triggered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        let triggered = CompletionMutationGuardResult {
            expected_mutation: true,
            attempted_mutation: false,
            triggered: true,
        };
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, triggered, dir.path(), None, None).await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_requires_an_approved_externally_supplied_reviewer_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
            .with_reviewer_result(ReviewerResult {
                approved: true,
                detail: Some("looks good".to_string()),
            });
        assert_eq!(contract.required_level, AcceptanceStatus::Reviewed);
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path(), None, None)
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Reviewed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_rejected_when_reviewer_result_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        contract.required_level = AcceptanceStatus::Reviewed; // demand Reviewed but attach no result
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path(), None, None)
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_rejected_when_reviewer_disapproved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![])
            .with_reviewer_result(ReviewerResult {
                approved: false,
                detail: Some("needs more work".to_string()),
            });
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path(), None, None)
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert!(ledger.detail.expect("detail").contains("needs more work"));
    }

    /// G79 on the LIVE gate: `run_sync` calls THIS `evaluate_acceptance`, whose `Checked` rung
    /// parses the child's report through `model::parse_acceptance_report`. A child that answered in
    /// snake_case, wrote `Done` instead of `satisfied`, sent a lone object where an array belongs
    /// and a bare string where a `string[]` belongs used to fail every declared criterion and every
    /// declared evidence kind — a rejected run for a purely cosmetic mismatch. After G79 the same
    /// answer passes, and the criterion id matches across `c 1` / `C_1` spellings.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_gate_accepts_an_aliased_child_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]).with_policy(
            model::normalize_criteria(
                &[model::CriterionInput::Gate(model::AcceptanceGate {
                    id: Some("C 1".to_string()),
                    must: Some("add a regression test".to_string()),
                    evidence: None,
                    severity: None,
                })],
                &[],
            ),
            vec![
                model::AcceptanceEvidenceKind::ChangedFiles,
                model::AcceptanceEvidenceKind::TestsAdded,
                model::AcceptanceEvidenceKind::CommandsRun,
                model::AcceptanceEvidenceKind::ManualNotes,
            ],
            None,
            Vec::new(),
        );
        let output = r#"done
```acceptance_report
{
  "criteria_satisfied": {"id": "c_1", "status": "Done", "evidence": "added tests/regression.rs"},
  "changed_files": "src/file.rs",
  "tests_added_or_updated": ["tests/regression.rs"],
  "commands_run": {"command": "cargo test", "result": "OK", "summary": "green"},
  "manual_notes": "nothing else"
}
```"#;
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some(output),
            no_guard_trigger(),
            dir.path(),
            None,
            None,
        )
        .await;
        assert_eq!(
            ledger.status,
            AcceptanceStatus::Checked,
            "aliased report must satisfy the gate; detail was {:?}",
            ledger.detail
        );
        assert_eq!(ledger.detail, None);
    }

    // ---------------------------------------------------------------------------------------
    // G78 — `reviewed` is not a requestable level, on the LIVE wire-lowering path every
    // execution surface shares (`lower_acceptance_input` -> `model::validate_acceptance_input`).
    // ---------------------------------------------------------------------------------------

    #[test]
    fn lowering_rejects_reviewed_as_a_requestable_level_in_both_wire_forms() {
        let bare = lower_acceptance_input(&serde_json::json!("reviewed"))
            .expect_err("a bare `reviewed` level is rejected at v0.43.0");
        assert_eq!(
            bare,
            format!("acceptance {}", model::EXPLICIT_REVIEWED_UNAVAILABLE)
        );
        // The message must point the caller at the replacement mechanism, not merely refuse.
        assert!(bare.contains("acceptance.review.required"));

        let object = lower_acceptance_input(&serde_json::json!({"level": "reviewed"}))
            .expect_err("an object-form `reviewed` level is rejected at v0.43.0");
        assert_eq!(
            object,
            format!("acceptance.level {}", model::EXPLICIT_REVIEWED_UNAVAILABLE)
        );
    }

    /// The advertise-vs-dispatch invariant: upstream v0.43.0 deliberately KEEPS `"reviewed"` in the
    /// advertised `AcceptanceOverride` enum (`schemas.ts:83-88`, marked `deprecated`) precisely so
    /// this preflight message can explain itself. A schema that stopped advertising it would leave
    /// the model guessing; a dispatch that accepted it would reinstate the deleted level.
    #[test]
    fn reviewed_is_still_advertised_so_the_rejection_can_explain_itself() {
        let rendered = crate::extension::sj_acceptance_override().to_string();
        assert!(
            rendered.contains("\"reviewed\""),
            "the acceptance enum must still advertise `reviewed` so preflight can explain it"
        );
        assert!(lower_acceptance_input(&serde_json::json!("reviewed")).is_err());
    }

    /// The advertise-vs-dispatch invariant, driven over the SCHEMA rather than a hand-written list:
    /// every string value `sj_acceptance_override` offers the model must be one `lower_acceptance_input`
    /// actually accepts — with exactly one upstream-sanctioned exception, `"reviewed"`, which is
    /// advertised in its own `deprecated` branch solely so the refusal can explain itself
    /// (`schemas.ts:83-88` @v0.43.0).
    ///
    /// G78 narrowed the dispatch (bare `"none"` and `"verified"` became hard errors,
    /// `acceptance.ts:183-184`) without narrowing the schema, so the tool advertised two values it
    /// would then refuse. Upstream narrowed both together: `schemas.ts:82` is exactly
    /// `["auto", "attested", "checked"]`.
    #[test]
    fn every_advertised_acceptance_level_is_one_the_dispatch_accepts() {
        let schema = crate::extension::sj_acceptance_override();
        let branches = schema
            .get("anyOf")
            .and_then(serde_json::Value::as_array)
            .expect("AcceptanceOverride is an anyOf");

        let mut advertised: Vec<String> = Vec::new();
        for branch in branches {
            if branch.get("type").and_then(serde_json::Value::as_str) != Some("string") {
                continue;
            }
            let deprecated = branch
                .get("deprecated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            for value in branch
                .get("enum")
                .and_then(serde_json::Value::as_array)
                .expect("a string branch carries an enum")
            {
                let value = value.as_str().expect("enum entries are strings");
                if deprecated {
                    // The sanctioned exception: advertised, refused, and the refusal EXPLAINS.
                    let err = lower_acceptance_input(&serde_json::json!(value)).expect_err(
                        "a deprecated advertised level must still be refused by the dispatch",
                    );
                    assert!(
                        err.contains(model::EXPLICIT_REVIEWED_UNAVAILABLE),
                        "the deprecated `{value}` must be refused with the explanatory text, got {err}"
                    );
                } else {
                    advertised.push(value.to_string());
                }
            }
        }

        assert_eq!(
            advertised,
            ["auto", "attested", "checked"],
            "schemas.ts:82 advertises exactly these three requestable levels"
        );
        for level in &advertised {
            assert!(
                lower_acceptance_input(&serde_json::json!(level)).is_ok(),
                "`{level}` is advertised to the model, so the dispatch must accept it"
            );
        }

        // And the two G78 newly-invalidated values must be gone from the advertised surface
        // entirely — not merely absent from the requestable branch.
        let rendered = schema.to_string();
        for gone in ["\"none\"", "\"verified\""] {
            assert!(
                !rendered.contains(gone),
                "{gone} is rejected by lower_acceptance_input and must not be advertised: {rendered}"
            );
        }
    }

    #[test]
    fn bare_none_and_verified_level_strings_are_rejected() {
        // `AcceptanceInput = Exclude<AcceptanceLevel, "none" | "verified"> | …` (`shared/types.ts:684-685`).
        let none = lower_acceptance_input(&serde_json::json!("none"))
            .expect_err("a bare `none` carries no reason and is rejected");
        assert!(none.contains("requires a reason"), "{none}");
        let verified = lower_acceptance_input(&serde_json::json!("verified"))
            .expect_err("a bare `verified` declares no runtime command and is rejected");
        assert!(
            verified.contains("requires object form with at least one runtime verify command"),
            "{verified}"
        );
        // The three still-requestable bare levels keep working.
        for level in ["auto", "attested", "checked"] {
            assert!(
                lower_acceptance_input(&serde_json::json!(level)).is_ok(),
                "`{level}` must remain requestable"
            );
        }
    }

    #[test]
    fn object_form_verified_without_runtime_commands_is_rejected() {
        let err = lower_acceptance_input(&serde_json::json!({"level": "verified"}))
            .expect_err("`verified` with no verify[] is rejected");
        assert!(
            err.contains("must contain at least one runtime command when level is verified"),
            "{err}"
        );
        let err = lower_acceptance_input(&serde_json::json!({"level": "verified", "verify": []}))
            .expect_err("`verified` with an EMPTY verify[] is rejected too");
        assert!(err.contains("at least one runtime command"), "{err}");
        // A real command is accepted, and the per-command validation still runs alongside.
        let ok = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [{"id": "gate", "command": "true"}],
        }))
        .expect("a verified policy with one command is valid")
        .expect("an explicit contract");
        assert_eq!(ok.required_level, AcceptanceStatus::Verified);
        let bad_item = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [{"command": "true"}],
        }))
        .expect_err("a verify[] entry with no id is still rejected");
        assert!(bad_item.contains("verify[0].id is required."), "{bad_item}");
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
            Some(model::ReviewSetting::Gate(gate)) if gate.required == Some(true)
                && gate.agent.as_deref() == Some("reviewer")
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn self_report_floor_distinguishes_claimed_from_attested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        // NotRequired contract is a no-op regardless, so force a Checked requirement that a
        // guard-pass alone satisfies, and inspect the self-report floor via a HIGHER achieved
        // level than what Checked alone would produce is not observable here (Checked already
        // dominates Claimed) — this test instead exercises the private classification path via
        // the achieved level when required_level is NotRequired-but-not-quite: use is_no_op
        // false by attaching required_level Checked with the guard failing so ONLY the self-
        // report floor and the checked evidence combine into `achieved`, and assert the ledger's
        // achieved status directly reflects the self-report floor when it's the higher of the two.
        let _ = contract; // documented above; real assertions happen in the two tests below.

        let bare_claim = self_report_floor(Some(
            "Done.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```",
        ));
        assert_eq!(bare_claim, AcceptanceStatus::Claimed);

        let attested = self_report_floor(Some(
            "Done.\n```acceptance-report\n{\"criteriaSatisfied\": true, \"changedFiles\": [\"a.rs\"]}\n```",
        ));
        assert_eq!(attested, AcceptanceStatus::Attested);

        let nothing = self_report_floor(Some("Just a plain answer, no report block."));
        assert_eq!(nothing, AcceptanceStatus::NotRequired);

        let none_at_all = self_report_floor(None);
        assert_eq!(none_at_all, AcceptanceStatus::NotRequired);

        let _ = dir; // no subprocess needed in this particular test; tempdir kept for symmetry.
    }

    // ---------------------------------------------------------------------------------------
    // apply_post_hoc_correction (R-SA-033)
    // ---------------------------------------------------------------------------------------

    #[test]
    fn explicit_rejected_contract_on_a_clean_run_corrects_exit_code_and_appends_error() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("verify[] command failed".to_string()),
            verify_results: Vec::new(),
        };
        let correction =
            apply_post_hoc_correction(&ledger, true, clean_gate(), Some("prior warning"));
        assert!(correction.corrected);
        assert_eq!(correction.exit_code, ACCEPTANCE_REJECTED_EXIT_CODE);
        assert_ne!(correction.exit_code, 0);
        let error = correction.error.expect("error must be set");
        assert!(error.contains("prior warning"));
        assert!(error.contains("verify[] command failed"));
    }

    #[test]
    fn heuristic_inferred_rejected_contract_never_corrects_exit_code() {
        // R-SA-033: "AND the acceptance contract was explicit AND..." — a heuristic contract's
        // rejection must be recorded on the ledger but must NEVER flip an otherwise-0 exit code.
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("checked: guard triggered".to_string()),
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, false, clean_gate(), None);
        assert!(!correction.corrected);
        assert_eq!(correction.exit_code, 0);
        assert!(correction.error.is_none());
    }

    #[test]
    fn a_non_rejected_ledger_never_triggers_a_correction() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Verified,
            evidence_status: model::AcceptanceEvidenceStatus::Verified,
            detail: None,
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, true, clean_gate(), None);
        assert!(!correction.corrected);
        assert_eq!(correction.exit_code, 0);
    }

    #[test]
    fn a_dirty_gate_never_triggers_a_correction_even_if_the_ledger_says_rejected() {
        // Defensive re-check: even a stale/inconsistent Rejected ledger must not correct a run
        // whose gate was not actually clean at the time this function runs (arch-SA §6.3.4: "a
        // run that already failed the completion guard is not corrected a second time").
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("stale".to_string()),
            verify_results: Vec::new(),
        };
        let dirty_gate = CleanCompletionGate {
            exit_code: 3,
            detached: false,
            interrupted: false,
            timed_out: false,
        };
        let correction = apply_post_hoc_correction(&ledger, true, dirty_gate, None);
        assert!(!correction.corrected);
        assert_eq!(
            correction.exit_code, 3,
            "the ORIGINAL nonzero exit code must be preserved unchanged, not overwritten"
        );
    }

    #[test]
    fn correction_with_no_prior_error_produces_a_clean_standalone_message() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: Some("no verify[] commands were declared".to_string()),
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, true, clean_gate(), None);
        assert!(correction.corrected);
        let error = correction.error.expect("error set");
        assert!(error.starts_with("acceptance rejected:"));
        assert!(!error.contains(';'), "no prior error to join with a separator");
    }

    #[test]
    fn correction_falls_back_to_a_generic_reason_when_ledger_has_no_detail() {
        let ledger = AcceptanceLedger {
            status: AcceptanceStatus::Rejected,
            evidence_status: model::AcceptanceEvidenceStatus::Rejected,
            detail: None,
            verify_results: Vec::new(),
        };
        let correction = apply_post_hoc_correction(&ledger, true, clean_gate(), None);
        assert!(correction.corrected);
        assert!(
            correction
                .error
                .expect("error")
                .contains("acceptance criteria were not met")
        );
    }

    // --------------------------------------------------------------------------------------
    // G82: `parseAcceptanceReportSources` (`acceptance.ts:753-772`), enum-lattice side
    // --------------------------------------------------------------------------------------

    /// The lattice-side [`select_acceptance_report_source`] has its own copy of upstream's
    /// primary/secondary rule and — unlike `model::parse_acceptance_report_sources` — had no unit
    /// test of it. The live `file-only` integration test cannot supply one: in that mode the run's
    /// `final_output` is absent by the time the gate runs, so the secondary source is `None` and
    /// BOTH the `authoritative` ordering and the primary-is-decisive rule collapse to the same
    /// answer no matter which way they are written.
    #[test]
    fn the_authoritative_file_is_searched_first_and_a_primary_defect_is_never_papered_over() {
        let file_report =
            "artifact\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-file\"}\n```";
        let text_report =
            "receipt\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-text\"}\n```";
        let path = Path::new("out.md");

        // Not authoritative: the assistant output is primary. It has no report, so the file — the
        // secondary — supplies it.
        assert_eq!(
            select_acceptance_report_source(
                Some("no report here"),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: false,
                }),
            ),
            Some(file_report)
        );

        // Not authoritative, and BOTH carry a report: the assistant output wins.
        assert_eq!(
            select_acceptance_report_source(
                Some(text_report),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: false,
                }),
            ),
            Some(text_report)
        );

        // `outputMode: "file-only"` makes the file authoritative: it is searched FIRST and wins
        // even though the assistant output also carries a report.
        assert_eq!(
            select_acceptance_report_source(
                Some(text_report),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: true,
                }),
            ),
            Some(file_report)
        );

        // A MALFORMED report in the primary source is a defect to surface, not a miss to paper
        // over with the secondary — only a genuinely absent report falls through.
        let malformed = "receipt\n```acceptance-report\n{\"criteriaSatisfied\": [{\"id\": \"c1\"}]}\n```";
        assert_eq!(
            select_acceptance_report_source(
                Some(malformed),
                Some(&AcceptanceFileOutput {
                    content: file_report,
                    path,
                    authoritative: false,
                }),
            ),
            Some(malformed),
            "a defective primary must not be replaced by the secondary"
        );
    }

    /// The full truth table for [`select_acceptance_report_source`], because the case above leaves
    /// three of its inputs unexercised and each hides an independent rule:
    ///
    /// * the defect rule is only ever seen with the swap OFF, so `authoritative` + a DEFECTIVE
    ///   file reads the same as `authoritative` + a valid one;
    /// * the fall-through is only ever seen with the swap OFF, so `authoritative` + a file with no
    ///   report at all never demonstrates that the assistant output is still consulted;
    /// * `file_output: None` is never passed, so the `secondary.or(primary)` tail — the branch that
    ///   keeps the run's own output rather than returning nothing — is never reached.
    ///
    /// Each source string's parse classification is asserted FIRST, so a change to the report
    /// grammar can never silently turn a "defective" fixture into an "absent" one and quietly
    /// reinterpret every row below it.
    #[test]
    fn the_report_source_truth_table_is_exhaustive_over_authoritative_and_parse_outcome() {
        const VALID_TEXT: &str =
            "receipt\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-text\"}\n```";
        const VALID_FILE: &str =
            "artifact\n```acceptance-report\n{\"criteriaSatisfied\": [], \"diffSummary\": \"from-file\"}\n```";
        const DEFECTIVE: &str =
            "artifact\n```acceptance-report\n{\"criteriaSatisfied\": [{\"id\": \"c1\"}]}\n```";
        const ABSENT_TEXT: &str = "no report in the assistant output";
        const ABSENT_FILE: &str = "no report in the configured output file";

        // --- the three parse classifications the selection rule discriminates on ---
        for valid in [VALID_TEXT, VALID_FILE] {
            assert!(
                model::parse_acceptance_report(valid).report.is_some(),
                "fixture must parse to a report: {valid:?}"
            );
        }
        let defective = model::parse_acceptance_report(DEFECTIVE);
        assert!(defective.report.is_none(), "fixture must not parse");
        assert_ne!(
            defective.error.as_deref(),
            Some(model::ACCEPTANCE_REPORT_NOT_FOUND),
            "a DEFECTIVE fixture must report a defect, never `not found`"
        );
        for absent in [ABSENT_TEXT, ABSENT_FILE] {
            assert_eq!(
                model::parse_acceptance_report(absent).error.as_deref(),
                Some(model::ACCEPTANCE_REPORT_NOT_FOUND),
                "fixture must be a genuine MISS: {absent:?}"
            );
        }

        let path = Path::new("out.md");
        let file = |content: &'static str, authoritative: bool| AcceptanceFileOutput {
            content,
            path,
            authoritative,
        };

        // --- `authoritative` OFF: the assistant output is primary ---
        // decisive primary wins over a decisive secondary...
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(VALID_FILE, false))),
            Some(VALID_TEXT)
        );
        // ...including a DEFECTIVE primary, which is a defect to surface, not a miss to paper over.
        assert_eq!(
            select_acceptance_report_source(Some(DEFECTIVE), Some(&file(VALID_FILE, false))),
            Some(DEFECTIVE)
        );
        // only a genuine MISS falls through.
        assert_eq!(
            select_acceptance_report_source(Some(ABSENT_TEXT), Some(&file(VALID_FILE, false))),
            Some(VALID_FILE)
        );

        // --- `authoritative` ON (`outputMode: "file-only"`): the file is primary ---
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(VALID_FILE, true))),
            Some(VALID_FILE)
        );
        // The defect rule, with the swap ON: a DEFECTIVE authoritative file is still decisive, so
        // the assistant output must NOT rescue it.
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(DEFECTIVE, true))),
            Some(DEFECTIVE),
            "a defective authoritative file must not be replaced by the assistant output"
        );
        // The fall-through, with the swap ON: an authoritative file with NO report at all is a
        // miss, so the assistant output is consulted after it.
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), Some(&file(ABSENT_FILE, true))),
            Some(VALID_TEXT),
            "an authoritative file that carries no report must fall through to the output"
        );

        // --- no secondary at all: `secondary.or(primary)` keeps the run's own output ---
        assert_eq!(
            select_acceptance_report_source(Some(ABSENT_TEXT), None),
            Some(ABSENT_TEXT),
            "with no file source, a missing report must not discard the run's own output"
        );
        assert_eq!(
            select_acceptance_report_source(Some(VALID_TEXT), None),
            Some(VALID_TEXT)
        );
        assert_eq!(select_acceptance_report_source(None, None), None);
        // A missing assistant output still lets a non-authoritative file supply the report.
        assert_eq!(
            select_acceptance_report_source(None, Some(&file(VALID_FILE, false))),
            Some(VALID_FILE)
        );
        // Neither source carries a report: the (identically "not found") secondary is returned.
        assert_eq!(
            select_acceptance_report_source(Some(ABSENT_TEXT), Some(&file(ABSENT_FILE, false))),
            Some(ABSENT_FILE)
        );
        assert_eq!(
            select_acceptance_report_source(None, Some(&file(ABSENT_FILE, true))),
            Some(ABSENT_FILE)
        );
    }
}

// ================================================================================================
// C12: faithful port of the pi acceptance criteria/evidence model
// (`pi-subagents/src/runs/shared/acceptance.ts` + `src/shared/types.ts:248-385`).
//
// The enum-lattice API above (`AcceptanceStatus`/`AcceptanceContract`/`evaluate_acceptance`) is the
// crate's original acceptance subsystem, wired into `exec/mod.rs::run_sync`. This module is the
// diagnosed-missing C12 port: the criteria/evidence/report model, level inference
// (`inferLevel` acceptance.ts:77-147 / `resolveEffectiveAcceptance` acceptance.ts:344-401), report-JSON parse+validate,
// evidence checks incl. the real `git status --short` no-staged-files check, `stripAcceptanceReport`
// (acceptance.ts:774-795), and the async `evaluateAcceptance` ledger with REAL `verify[]` subprocess
// execution. Names live under `model` so they never collide with the enum-lattice API's own
// `AcceptanceLedger`/`evaluate_acceptance`.
//
// UPSTREAM HAS ONE ACCEPTANCE IMPLEMENTATION. The two-API split is this crate's own accretion and
// is unfinished port work, not a design. It is being COLLAPSED onto this module, layer by layer,
// and the collapse is currently complete for:
//
//   * the verify RUNNER — one `run_verify_command`/`run_memoized_verify_command`, here, called by
//     both gates. The enum-lattice copy (`run_one_verify_command`) is gone.
//   * the verify RESULT — one `AcceptanceVerifyResult`, here, carrying every field upstream
//     stamps on `ledger.verifyRuns[]`. The enum-lattice copy (`VerifyCommandResult`) is gone;
//     `super::AcceptanceLedger::verify_results` holds THIS type.
//   * `validateAcceptanceInput` — one implementation, here; `discovery/chains.rs` delegates to it
//     rather than carrying the second transcription it used to.
//   * `evidenceStatus` — one meaning, here (`AcceptanceEvidenceStatus`); the enum-lattice ledger
//     carries the same type, projected via `super::AcceptanceStatus::evidence_status`.
//
// What has NOT collapsed: the LEDGER struct and the verdict that fills it. `super::AcceptanceLedger`
// still computes `status`/`detail` from `super::AcceptanceContract`'s required-level lattice rather
// than from `AcceptanceLevel` + `runtimeChecks`, and it does not carry upstream's `runtimeChecks`,
// `childReport`, `childReportParseError`, `reviewResult`, `explicit`, `inferredReason` or
// `criteria`. Finishing the collapse means `exec/mod.rs::run_sync` calling `evaluate_acceptance`
// here and `SingleResult.acceptance` becoming `AcceptanceLedger` here.
// ================================================================================================
pub mod model {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use serde_json::Value;

    use crate::exec::completion_guard::{any_word_boundary, word_boundary_contains};

    // --------------------------------------------------------------------------------------------
    // Enums (shared/types.ts:248-373)
    // --------------------------------------------------------------------------------------------

    /// `AcceptanceLevel` (`shared/types.ts:639` @v0.43.0:
    /// `"auto" | "none" | "attested" | "checked" | "verified"`) — `auto` is the "infer" sentinel;
    /// every other variant is a concrete provenance level. Ordering rank is
    /// `none < attested < checked < verified` ([`level_rank`]); `Auto` has no rank.
    ///
    /// **`reviewed` is NOT a level.** Up to v0.34.0 the union carried a sixth member `"reviewed"`;
    /// v0.43.0 removed it, because `reviewed` is an ACHIEVED ledger status (something an
    /// independent reviewer produces) and never a requestable acceptance level. A policy that wants
    /// independent review declares `acceptance.review.required` instead, and
    /// `validateAcceptanceInput` now rejects the string `"reviewed"` outright with
    /// [`EXPLICIT_REVIEWED_UNAVAILABLE`] (`acceptance.ts:54,181,195-196`). The achieved status still
    /// exists — see [`AcceptanceLedgerStatus::Reviewed`], which is deliberately NOT an
    /// [`AcceptanceEvidenceStatus`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum AcceptanceLevel {
        Auto,
        None,
        Attested,
        Checked,
        Verified,
    }

    impl AcceptanceLevel {
        #[must_use]
        pub fn as_str(self) -> &'static str {
            match self {
                AcceptanceLevel::Auto => "auto",
                AcceptanceLevel::None => "none",
                AcceptanceLevel::Attested => "attested",
                AcceptanceLevel::Checked => "checked",
                AcceptanceLevel::Verified => "verified",
            }
        }
    }

    /// `LEVEL_RANK` (acceptance.ts:28-33 @v0.43.0) — `None` for `Auto` (unranked).
    fn level_rank(level: AcceptanceLevel) -> Option<u8> {
        match level {
            AcceptanceLevel::Auto => Option::None,
            AcceptanceLevel::None => Some(0),
            AcceptanceLevel::Attested => Some(1),
            AcceptanceLevel::Checked => Some(2),
            AcceptanceLevel::Verified => Some(3),
        }
    }

    /// `AcceptanceEvidenceKind` (shared/types.ts:250-259).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum AcceptanceEvidenceKind {
        ChangedFiles,
        TestsAdded,
        CommandsRun,
        ValidationOutput,
        ResidualRisks,
        NoStagedFiles,
        DiffSummary,
        ReviewFindings,
        ManualNotes,
    }

    impl AcceptanceEvidenceKind {
        #[must_use]
        pub fn as_str(self) -> &'static str {
            match self {
                AcceptanceEvidenceKind::ChangedFiles => "changed-files",
                AcceptanceEvidenceKind::TestsAdded => "tests-added",
                AcceptanceEvidenceKind::CommandsRun => "commands-run",
                AcceptanceEvidenceKind::ValidationOutput => "validation-output",
                AcceptanceEvidenceKind::ResidualRisks => "residual-risks",
                AcceptanceEvidenceKind::NoStagedFiles => "no-staged-files",
                AcceptanceEvidenceKind::DiffSummary => "diff-summary",
                AcceptanceEvidenceKind::ReviewFindings => "review-findings",
                AcceptanceEvidenceKind::ManualNotes => "manual-notes",
            }
        }

        /// Parse one authored `evidence[]` entry. `None` for anything not in
        /// `AcceptanceEvidenceKind` (shared/types.ts:250-259) — [`validate_acceptance_input`] has already
        /// rejected such an entry with `evidence[i] is not a supported evidence kind.` by the time
        /// [`super::lower_acceptance_input`] calls this, so the `None` arm is a total-function
        /// guard rather than a reachable policy path.
        #[must_use]
        pub fn from_wire(text: &str) -> Option<Self> {
            match text {
                "changed-files" => Some(AcceptanceEvidenceKind::ChangedFiles),
                "tests-added" => Some(AcceptanceEvidenceKind::TestsAdded),
                "commands-run" => Some(AcceptanceEvidenceKind::CommandsRun),
                "validation-output" => Some(AcceptanceEvidenceKind::ValidationOutput),
                "residual-risks" => Some(AcceptanceEvidenceKind::ResidualRisks),
                "no-staged-files" => Some(AcceptanceEvidenceKind::NoStagedFiles),
                "diff-summary" => Some(AcceptanceEvidenceKind::DiffSummary),
                "review-findings" => Some(AcceptanceEvidenceKind::ReviewFindings),
                "manual-notes" => Some(AcceptanceEvidenceKind::ManualNotes),
                _ => Option::None,
            }
        }
    }

    /// `"required" | "recommended"` (shared/types.ts:265).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum GateSeverity {
        Required,
        Recommended,
    }

    // --------------------------------------------------------------------------------------------
    // Config-input shapes (shared/types.ts:261-293)
    // --------------------------------------------------------------------------------------------

    /// One acceptance criterion as authored: either a bare `must` string or a full [`AcceptanceGate`]
    /// (types.ts `Array<string | AcceptanceGate>`).
    #[derive(Debug, Clone, PartialEq)]
    pub enum CriterionInput {
        Text(String),
        Gate(AcceptanceGate),
    }

    /// `AcceptanceGate` (shared/types.ts:261-266).
    #[derive(Debug, Clone, PartialEq)]
    pub struct AcceptanceGate {
        pub id: Option<String>,
        pub must: Option<String>,
        pub evidence: Option<Vec<AcceptanceEvidenceKind>>,
        pub severity: Option<GateSeverity>,
    }

    /// `AcceptanceVerifyCommand` (shared/types.ts:268-275).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcceptanceVerifyCommand {
        pub id: String,
        pub command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub env: Option<std::collections::BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub allow_failure: Option<bool>,
    }

    impl AcceptanceVerifyCommand {
        /// A bare shell command with no per-command overrides — every optional field left unset so
        /// [`run_verify_command`] applies the run-level `cwd`, the inherited
        /// environment and [`DEFAULT_VERIFY_TIMEOUT_MS`], exactly as an authored entry that
        /// declares only `{ id, command }` does.
        ///
        /// `id` defaults to the command text itself. Upstream *requires* an explicit `id`
        /// (`acceptance.ts:209` — `verify[i].id is required.`) and
        /// [`validate_acceptance_input`] enforces that before [`super::lower_acceptance_input`]
        /// runs, so this fallback is only ever reached by callers constructing a contract in Rust
        /// rather than from an authored `acceptance` param.
        #[must_use]
        pub fn shell(command: impl Into<String>) -> Self {
            let command = command.into();
            Self {
                id: command.clone(),
                command,
                timeout_ms: Option::None,
                cwd: Option::None,
                env: Option::None,
                allow_failure: Option::None,
            }
        }
    }

    impl From<String> for AcceptanceVerifyCommand {
        fn from(command: String) -> Self {
            Self::shell(command)
        }
    }

    impl From<&str> for AcceptanceVerifyCommand {
        fn from(command: &str) -> Self {
            Self::shell(command)
        }
    }

    /// A declared command compares equal to the bare command string it runs, so callers that only
    /// care about *which shell commands a contract will execute* (the property that mattered when
    /// [`super::VerifyCommand`] was still a `String`) keep expressing that directly. The
    /// per-command overrides are deliberately NOT part of this comparison — use the derived
    /// `PartialEq` on two `AcceptanceVerifyCommand`s for full structural equality.
    impl PartialEq<String> for AcceptanceVerifyCommand {
        fn eq(&self, other: &String) -> bool {
            self.command == *other
        }
    }

    impl PartialEq<&str> for AcceptanceVerifyCommand {
        fn eq(&self, other: &&str) -> bool {
            self.command == *other
        }
    }

    /// `AcceptanceReviewGate` (shared/types.ts:277-281).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcceptanceReviewGate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub focus: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub required: Option<bool>,
    }

    /// `AcceptanceReviewGate | false` (shared/types.ts:288) — `Disabled` is the `false` shorthand.
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(untagged)]
    pub enum ReviewSetting {
        Disabled(bool),
        Gate(AcceptanceReviewGate),
    }

    /// `AcceptanceConfig` (shared/types.ts:283-291).
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct AcceptanceConfig {
        pub level: Option<AcceptanceLevel>,
        pub criteria: Option<Vec<CriterionInput>>,
        pub evidence: Option<Vec<AcceptanceEvidenceKind>>,
        pub verify: Option<Vec<AcceptanceVerifyCommand>>,
        pub review: Option<ReviewSetting>,
        pub stop_rules: Option<Vec<String>>,
        pub reason: Option<String>,
    }

    /// `AcceptanceInput = AcceptanceLevel | false | AcceptanceConfig` (shared/types.ts:293).
    #[derive(Debug, Clone, PartialEq)]
    pub enum AcceptanceInput {
        Level(AcceptanceLevel),
        Disabled,
        Config(AcceptanceConfig),
    }

    // --------------------------------------------------------------------------------------------
    // Resolved shapes (shared/types.ts:295-312)
    // --------------------------------------------------------------------------------------------

    /// `ResolvedAcceptanceGate` (shared/types.ts:295-300).
    #[derive(Debug, Clone, PartialEq)]
    pub struct ResolvedAcceptanceGate {
        pub id: String,
        pub must: String,
        pub evidence: Vec<AcceptanceEvidenceKind>,
        pub severity: GateSeverity,
    }

    /// `ResolvedAcceptanceConfig` (shared/types.ts:302-312).
    #[derive(Debug, Clone, PartialEq)]
    pub struct ResolvedAcceptanceConfig {
        pub level: AcceptanceLevel,
        pub explicit: bool,
        pub inferred_reason: Vec<String>,
        pub criteria: Vec<ResolvedAcceptanceGate>,
        pub evidence: Vec<AcceptanceEvidenceKind>,
        pub verify: Vec<AcceptanceVerifyCommand>,
        pub review: Option<ReviewSetting>,
        pub stop_rules: Vec<String>,
        pub reason: Option<String>,
    }

    // --------------------------------------------------------------------------------------------
    // Report / runtime-check / ledger shapes (shared/types.ts:314-385)
    // --------------------------------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum CriterionStatus {
        Satisfied,
        NotSatisfied,
        NotApplicable,
    }

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct CriterionReport {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub id: Option<String>,
        pub status: CriterionStatus,
        pub evidence: String,
    }

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum CommandRunResult {
        Passed,
        Failed,
        NotRun,
    }

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct CommandRunReport {
        pub command: String,
        pub result: CommandRunResult,
        pub summary: String,
    }

    /// `AcceptanceReport` (shared/types.ts:314-334).
    #[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcceptanceReport {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub criteria_satisfied: Option<Vec<CriterionReport>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub changed_files: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tests_added_or_updated: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub commands_run: Option<Vec<CommandRunReport>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub validation_output: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub residual_risks: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub no_staged_files: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub diff_summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub review_findings: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub manual_notes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub notes: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum RuntimeCheckStatus {
        Passed,
        Failed,
        NotApplicable,
    }

    /// `AcceptanceRuntimeCheck` (shared/types.ts:338-342).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct AcceptanceRuntimeCheck {
        pub id: String,
        pub status: RuntimeCheckStatus,
        pub message: String,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum VerifyRunStatus {
        Passed,
        Failed,
        TimedOut,
        AllowedFailure,
    }

    /// `AcceptanceVerifyResult` (`shared/types.ts:736-758` @v0.43.0).
    ///
    /// The trailing seven fields are the memoization EVIDENCE upstream stamps onto every result
    /// that went through [`run_memoized_verify_command`] (`acceptance.ts:1106,1112,1128-1129`).
    /// They are all `Option` and all `skip_serializing_if`-omitted, exactly like upstream's `?:`
    /// members, so a result produced without a memo context serializes byte-for-byte as it did
    /// before this port.
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcceptanceVerifyResult {
        pub id: String,
        pub command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cwd: Option<String>,
        pub exit_code: Option<i32>,
        pub status: VerifyRunStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub stdout: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub stderr: Option<String>,
        pub duration_ms: u128,
        /// `artifactPath` (`shared/types.ts:745`) — where this run's memo artifact was read from/written
        /// to. Cleared (`delete evidenced.artifactPath`, `acceptance.ts:1129`) when the write
        /// itself failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub artifact_path: Option<String>,
        /// `cacheKey` (`shared/types.ts:746`) — the sha256 over the memo identity (command text, repo-
        /// relative cwd, declared env key names, full effective-env hash, timeout, `allowFailure`,
        /// `HEAD`, working-tree diff hash).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cache_key: Option<String>,
        /// `memoized` (`shared/types.ts:747`) — `Some(true)` when this result was REPLAYED from the memo
        /// artifact instead of executed, `Some(false)` when it was executed under an active memo
        /// context, `None` when no memo context applied at all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub memoized: Option<bool>,
        /// `envKeys` (`shared/types.ts:748`) — the sorted key names of the command's OWN declared `env`
        /// (`Object.keys(command.env ?? {}).sort()`, `acceptance.ts:1088`). Names only; no values,
        /// so a secret-bearing override never reaches the ledger.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub env_keys: Option<Vec<String>>,
        /// `envHash` (`shared/types.ts:749`) — sha256 over the whole EFFECTIVE environment
        /// (`acceptance.ts:1089`), so a changed secret invalidates the memo without the value ever
        /// being written down.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub env_hash: Option<String>,
        /// `workspaceState` (`shared/types.ts:750-756`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub workspace_state: Option<VerifyWorkspaceState>,
        /// `artifactError` (`shared/types.ts:757`) — set when the memo artifact could not be written
        /// (`acceptance.ts:1128`). Never fails the verification itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub artifact_error: Option<String>,
    }

    /// `VerifyWorkspaceState.kind` (`acceptance.ts:1039`) — the single discriminant upstream
    /// declares. A workspace that is not a git checkout produces no state at all (and therefore no
    /// memoization), rather than a second variant.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum VerifyWorkspaceKind {
        GitTracked,
    }

    /// `VerifyWorkspaceState` (`acceptance.ts:1038-1044`): the identity of the working tree a
    /// verify command's result is memoized AGAINST. `head` + `diff_hash` together pin both the
    /// committed and the uncommitted state, so any edit to the tree invalidates every memo.
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct VerifyWorkspaceState {
        pub kind: VerifyWorkspaceKind,
        pub repo_root: String,
        pub cwd_relative: String,
        pub head: String,
        pub diff_hash: String,
    }

    /// `AcceptanceReviewResult["status"]` (`shared/types.ts:761` @v0.43.0:
    /// `"review-required" | "reviewed" | "blockers"`).
    ///
    /// v0.34.0 spelled this `"no-blockers" | "blockers" | "needs-parent-decision"`. v0.43.0 renamed
    /// both non-`blockers` members so the review outcome shares the LEDGER's own vocabulary: a
    /// reviewer that signed off yields `reviewed` (which is exactly the ledger status the run then
    /// takes) and an absent/incomplete review yields `review-required` (likewise). See
    /// [`evaluate_acceptance`]'s review block, `acceptance.ts:1318-1336`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum ReviewResultStatus {
        ReviewRequired,
        Reviewed,
        Blockers,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum ReviewFindingSeverity {
        Blocker,
        NonBlocking,
    }

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct ReviewFinding {
        pub severity: ReviewFindingSeverity,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub file: Option<String>,
        pub issue: String,
        pub rationale: String,
    }

    /// `AcceptanceReviewResult` (shared/types.ts:355-363).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct AcceptanceReviewResult {
        pub status: ReviewResultStatus,
        pub findings: Vec<ReviewFinding>,
    }

    /// `AcceptanceEvidenceStatus` (`shared/types.ts:770-777` @v0.43.0) — the strictly EVIDENCE-derived
    /// half of the ledger's status: how far the child's own report plus the orchestrator's own
    /// structural/verify checks carried this run, with review deliberately excluded.
    ///
    /// v0.43.0 split this out of `AcceptanceLedgerStatus` so a run whose evidence genuinely reached
    /// `verified` still reads as `verified` on `evidenceStatus` even while `status` sits at
    /// `review-required` waiting for an independent reviewer. Before the split there was one field,
    /// so "the review has not happened yet" ERASED the evidence level that had already been earned.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum AcceptanceEvidenceStatus {
        Pending,
        NotRequired,
        Claimed,
        Attested,
        Checked,
        Verified,
        Rejected,
    }

    /// `AcceptanceLedgerStatus` (`shared/types.ts:779-783` @v0.43.0) —
    /// `AcceptanceEvidenceStatus | "review-required" | "reviewed" | "accepted"`. Rust has no union
    /// type, so the evidence members are restated here and [`AcceptanceEvidenceStatus`] converts
    /// into this enum via [`From`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum AcceptanceLedgerStatus {
        Pending,
        NotRequired,
        Claimed,
        Attested,
        Checked,
        Verified,
        Rejected,
        ReviewRequired,
        Reviewed,
        Accepted,
    }

    impl From<AcceptanceEvidenceStatus> for AcceptanceLedgerStatus {
        fn from(status: AcceptanceEvidenceStatus) -> Self {
            match status {
                AcceptanceEvidenceStatus::Pending => AcceptanceLedgerStatus::Pending,
                AcceptanceEvidenceStatus::NotRequired => AcceptanceLedgerStatus::NotRequired,
                AcceptanceEvidenceStatus::Claimed => AcceptanceLedgerStatus::Claimed,
                AcceptanceEvidenceStatus::Attested => AcceptanceLedgerStatus::Attested,
                AcceptanceEvidenceStatus::Checked => AcceptanceLedgerStatus::Checked,
                AcceptanceEvidenceStatus::Verified => AcceptanceLedgerStatus::Verified,
                AcceptanceEvidenceStatus::Rejected => AcceptanceLedgerStatus::Rejected,
            }
        }
    }

    /// `AcceptanceLedger` (`shared/types.ts:785-800` @v0.43.0, subset actually populated by
    /// `evaluateAcceptance`).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcceptanceLedger {
        pub status: AcceptanceLedgerStatus,
        /// `evidenceStatus` (`shared/types.ts:787`) — moves in lockstep with `status` through the
        /// attestation/checked/verified rungs and is then FROZEN: `evaluateAcceptance`'s review
        /// block (`acceptance.ts:1318-1336`) rewrites only `status`, never this field.
        pub evidence_status: AcceptanceEvidenceStatus,
        pub explicit: bool,
        pub inferred_reason: Vec<String>,
        pub criteria: Vec<SerializableGate>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub child_report: Option<AcceptanceReport>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub child_report_parse_error: Option<String>,
        pub runtime_checks: Vec<AcceptanceRuntimeCheck>,
        pub verify_runs: Vec<AcceptanceVerifyResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub review_result: Option<AcceptanceReviewResult>,
    }

    /// Serializable projection of a [`ResolvedAcceptanceGate`] for the ledger (evidence rendered as
    /// wire strings).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct SerializableGate {
        pub id: String,
        pub must: String,
        pub evidence: Vec<String>,
        pub severity: String,
    }

    impl SerializableGate {
        fn from_gate(gate: &ResolvedAcceptanceGate) -> Self {
            Self {
                id: gate.id.clone(),
                must: gate.must.clone(),
                evidence: gate.evidence.iter().map(|k| k.as_str().to_string()).collect(),
                severity: match gate.severity {
                    GateSeverity::Required => "required".to_string(),
                    GateSeverity::Recommended => "recommended".to_string(),
                },
            }
        }
    }

    // --------------------------------------------------------------------------------------------
    // requiredEvidenceForLevel (acceptance.ts:55-67) + level inference (acceptance.ts:69-125)
    // --------------------------------------------------------------------------------------------

    /// `requiredEvidenceForLevel` (acceptance.ts:55-67).
    fn required_evidence_for_level(level: AcceptanceLevel) -> Vec<AcceptanceEvidenceKind> {
        use AcceptanceEvidenceKind::*;
        match level {
            AcceptanceLevel::None | AcceptanceLevel::Auto => Vec::new(),
            AcceptanceLevel::Attested => vec![ManualNotes, ResidualRisks],
            AcceptanceLevel::Checked => {
                vec![ChangedFiles, TestsAdded, CommandsRun, ResidualRisks, NoStagedFiles]
            }
            AcceptanceLevel::Verified => vec![
                ChangedFiles,
                TestsAdded,
                CommandsRun,
                ValidationOutput,
                ResidualRisks,
                NoStagedFiles,
            ],
        }
    }

    /// `SubagentRunMode` (shared/types.ts:231) — carried for parity with pi's `inferLevel` input even
    /// though the current heuristic does not branch on it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SubagentRunMode {
        Single,
        Parallel,
        Chain,
    }

    /// Input to [`resolve_effective_acceptance`] / `infer_level` (acceptance.ts:69-76, 265-273).
    #[derive(Debug, Clone, Default)]
    pub struct AcceptanceResolveInput {
        pub explicit: Option<AcceptanceInput>,
        pub agent_name: String,
        pub task: Option<String>,
        pub mode: Option<SubagentRunMode>,
        pub is_async: bool,
        pub dynamic: bool,
        pub dynamic_group: bool,
    }

    struct InferredLevel {
        level: AcceptanceLevel,
        reasons: Vec<String>,
        criteria: Vec<CriterionInput>,
        evidence: Vec<AcceptanceEvidenceKind>,
        review: Option<ReviewSetting>,
    }

    /// `inferLevel` (acceptance.ts:77-147 @v0.43.0) — regex-free word-boundary port (the classifier reuses
    /// `completion_guard`'s already-tested `word_boundary_contains`, exactly as the enum-lattice
    /// `heuristic_default` reuses `expects_implementation_mutation`).
    fn infer_level(input: &AcceptanceResolveInput) -> InferredLevel {
        let agent = input.agent_name.to_lowercase();
        let task = input.task.as_deref().unwrap_or("").to_lowercase();
        let mut reasons: Vec<String> = Vec::new();

        // `/\b(?:reviewer|oracle|scout|researcher|analyst)\b/` (`acceptance.ts:99` @ v0.43.0).
        //
        // Both edits to this alternation are VERSION LAG, not a port bug. At the ported baseline it
        // read `reviewer|scout|context-builder|researcher|analyst` (`acceptance.ts:80` @ v0.34.0),
        // which is exactly what this port originally carried — correctly. Upstream `83b9872`
        // ("fix: remove stale bundled roles") then dropped `context-builder` and added `oracle` in
        // the SAME edit; `git log -S` over this alternation returns that one commit and no other.
        // Both halves are applied together here for the same reason they were made together.
        let read_only_agent = any_word_boundary(
            &agent,
            &["reviewer", "oracle", "scout", "researcher", "analyst"],
        );
        // G83 — `const intent = classifyTaskMutationIntent(input.acceptanceRole ? "worker" :
        // input.agentName, input.task ?? "")` (`acceptance.ts:90`). This crate has no
        // `acceptanceRole` input, which is exactly upstream's `acceptanceRole === undefined`
        // branch, so the agent name is passed straight through.
        let intent = crate::exec::task_intent::classify_task_mutation_intent(
            &input.agent_name,
            input.task.as_deref().unwrap_or(""),
        );
        // `const readOnlyTask = intent.kind === "read-only" || (intent.kind === "unknown" &&
        // /\b(?:read[- ]only|review[- ]only|no edits|without edits|inspect|summari[sz]e)\b/.test(task))`
        // (`acceptance.ts:91-92`). The keyword probe is a FALLBACK for `unknown` only, and its
        // `do not edit`/`don't edit` entries moved into the classifier — a bare keyword scan
        // cannot tell `Do not edit files.` (blanket, read-only) from `Do not edit unrelated files;
        // implement the fix.` (scoped constraint on an implementation task), and used to call both
        // read-only.
        let read_only_task = intent == crate::exec::task_intent::TaskMutationIntent::ReadOnly
            || (intent == crate::exec::task_intent::TaskMutationIntent::Unknown
                && any_word_boundary(
                    &task,
                    &[
                        "read only",
                        "read-only",
                        "review only",
                        "review-only",
                        "no edits",
                        "without edits",
                        "inspect",
                        "summarise",
                        "summarize",
                    ],
                ));
        // `const taskMayWrite = readOnlyTask ? false : taskMayMutate(input.task ?? "") ||
        // intent.kind === "implementation" || rolePatchTask` (`acceptance.ts:97`), with
        // `rolePatchTask === false` because no acceptance role is declared (`:93-96`).
        let task_may_write = !read_only_task
            && (crate::exec::task_intent::task_may_mutate(input.task.as_deref().unwrap_or(""))
                || intent == crate::exec::task_intent::TaskMutationIntent::Implementation);
        // `const writeTask = taskMayWrite || (input.acceptanceRole === "writer" && !readOnlyTask)
        // || (input.acceptanceRole === undefined && /\bworker\b/.test(agent) && !readOnlyTask)`
        // (`acceptance.ts:100-102`).
        let write_task =
            task_may_write || (word_boundary_contains(&agent, "worker") && !read_only_task);
        // `const keywordRiskReadOnly = input.acceptanceRole === undefined ? intent.kind ===
        // "read-only" : inferredReadOnly` (`acceptance.ts:105`).
        let keyword_risk_read_only = intent == crate::exec::task_intent::TaskMutationIntent::ReadOnly;
        // /\b(?:release|migration|migrate|security|data[- ]loss|destructive|post-review|fix pass)\b/
        let risky_task = any_word_boundary(
            &task,
            &[
                "release",
                "migration",
                "migrate",
                "security",
                "data loss",
                "data-loss",
                "destructive",
                "post-review",
                "fix pass",
            ],
        );
        // `const risky = Boolean(input.async && writeTask) || (Boolean(input.dynamic) &&
        // !roleResolvesReadOnly) || (Boolean(input.dynamicGroup) && !roleResolvesReadOnly) ||
        // (!keywordRiskReadOnly && /…/.test(task))` (`acceptance.ts:106-109`);
        // `roleResolvesReadOnly` is `false` with no acceptance role declared (`:102`).
        let risky = (input.is_async && write_task)
            || input.dynamic
            || input.dynamic_group
            || (!keyword_risk_read_only && risky_task);

        if risky {
            reasons.push(
                if input.is_async {
                    "async write-capable or risky run"
                } else {
                    "risky write-capable run"
                }
                .to_string(),
            );
            if input.dynamic || input.dynamic_group {
                reasons.push("dynamic fanout context".to_string());
            }
            // `acceptance.ts:114-120` @v0.43.0 — the risky branch returns `level: "checked"` plus a
            // REQUIRED review gate. Up to v0.34.0 it returned `level: "reviewed"`; v0.43.0 deleted
            // that level entirely (see [`AcceptanceLevel`]), so the "an independent reviewer must
            // sign this off" half of the escalation now lives ONLY in `review`, never in `level`.
            return InferredLevel {
                level: AcceptanceLevel::Checked,
                reasons,
                criteria: vec![
                    CriterionInput::Text(
                        "Implement the requested change without widening scope".to_string(),
                    ),
                    CriterionInput::Text(
                        "Return evidence sufficient for an independent acceptance review".to_string(),
                    ),
                ],
                evidence: required_evidence_for_level(AcceptanceLevel::Checked),
                review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                    agent: Some("reviewer".to_string()),
                    focus: Option::None,
                    required: Some(true),
                })),
            };
        }
        if write_task && !read_only_task {
            reasons.push("write-capable worker/task".to_string());
            return InferredLevel {
                level: AcceptanceLevel::Checked,
                reasons,
                criteria: vec![CriterionInput::Text(
                    "Implement the requested change without widening scope".to_string(),
                )],
                evidence: required_evidence_for_level(AcceptanceLevel::Checked),
                review: Option::None,
            };
        }
        if read_only_agent || read_only_task {
            reasons.push(
                if read_only_agent {
                    "read-only/reviewer-style agent"
                } else {
                    "read-only task wording"
                }
                .to_string(),
            );
            return InferredLevel {
                level: AcceptanceLevel::Attested,
                reasons,
                criteria: vec![CriterionInput::Text(
                    "Return concrete findings with file paths and severity when applicable"
                        .to_string(),
                )],
                evidence: vec![
                    AcceptanceEvidenceKind::ReviewFindings,
                    AcceptanceEvidenceKind::ResidualRisks,
                ],
                review: Option::None,
            };
        }
        reasons.push("default lightweight attestation".to_string());
        InferredLevel {
            level: AcceptanceLevel::Attested,
            reasons,
            criteria: vec![CriterionInput::Text(
                "Return a concise result and residual risks when applicable".to_string(),
            )],
            evidence: vec![
                AcceptanceEvidenceKind::ManualNotes,
                AcceptanceEvidenceKind::ResidualRisks,
            ],
            review: Option::None,
        }
    }

    // --------------------------------------------------------------------------------------------
    // normalizeAcceptanceInput / resolveEffectiveAcceptance (acceptance.ts:127-302)
    // --------------------------------------------------------------------------------------------

    /// `normalizeAcceptanceInput` (acceptance.ts:149-154).
    #[must_use]
    pub fn normalize_acceptance_input(input: Option<&AcceptanceInput>) -> AcceptanceConfig {
        match input {
            Option::None | Some(AcceptanceInput::Level(AcceptanceLevel::Auto)) => AcceptanceConfig {
                level: Some(AcceptanceLevel::Auto),
                ..AcceptanceConfig::default()
            },
            Some(AcceptanceInput::Disabled) => AcceptanceConfig {
                level: Some(AcceptanceLevel::None),
                reason: Some("disabled by deprecated false shorthand".to_string()),
                ..AcceptanceConfig::default()
            },
            Some(AcceptanceInput::Level(level)) => AcceptanceConfig {
                level: Some(*level),
                ..AcceptanceConfig::default()
            },
            Some(AcceptanceInput::Config(config)) => config.clone(),
        }
    }

    /// `explicitAcceptanceCanDisable` (acceptance.ts:167-174).
    fn explicit_acceptance_can_disable(explicit: &AcceptanceConfig) -> bool {
        explicit.level == Some(AcceptanceLevel::None)
            && explicit
                .reason
                .as_deref()
                .is_some_and(|reason| !reason.trim().is_empty())
    }

    /// `normalizeCriteria` (acceptance.ts:330-342).
    ///
    /// Public because [`super::lower_acceptance_input`] resolves an authored `criteria[]` through
    /// this exact function on its way onto [`super::AcceptanceContract::criteria`] — the ONE
    /// normalization rule (id fallback `criterion-<n>`, evidence inheritance, blank-`must` drop)
    /// must not be re-implemented on the live path.
    #[must_use]
    pub fn normalize_criteria(
        criteria: &[CriterionInput],
        evidence: &[AcceptanceEvidenceKind],
    ) -> Vec<ResolvedAcceptanceGate> {
        criteria
            .iter()
            .enumerate()
            .map(|(index, criterion)| match criterion {
                CriterionInput::Text(must) => ResolvedAcceptanceGate {
                    id: format!("criterion-{}", index + 1),
                    must: must.clone(),
                    evidence: evidence.to_vec(),
                    severity: GateSeverity::Required,
                },
                CriterionInput::Gate(gate) => ResolvedAcceptanceGate {
                    id: gate
                        .id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("criterion-{}", index + 1)),
                    must: gate.must.clone().unwrap_or_default(),
                    evidence: gate.evidence.clone().unwrap_or_else(|| evidence.to_vec()),
                    severity: gate.severity.unwrap_or(GateSeverity::Required),
                },
            })
            .filter(|criterion| !criterion.must.trim().is_empty())
            .collect()
    }

    /// Order-preserving de-duplication of an evidence list (`[...new Set(...)]`,
    /// acceptance.ts:283-285). Shared with [`super::lower_acceptance_input`] so a policy declaring
    /// the same kind twice produces one prompt line and one runtime check, not two.
    #[must_use]
    pub fn unique_evidence(items: &[AcceptanceEvidenceKind]) -> Vec<AcceptanceEvidenceKind> {
        let mut seen: Vec<AcceptanceEvidenceKind> = Vec::new();
        for item in items {
            if !seen.contains(item) {
                seen.push(*item);
            }
        }
        seen
    }

    /// `resolveEffectiveAcceptance` (acceptance.ts:344-401) — including the explicit-vs-inferred MAX
    /// escalation and the "inference-escalated-to-reviewed" review-downgrade rule.
    #[must_use]
    pub fn resolve_effective_acceptance(input: &AcceptanceResolveInput) -> ResolvedAcceptanceConfig {
        let explicit = normalize_acceptance_input(input.explicit.as_ref());
        let inferred = infer_level(input);
        let explicit_level = explicit.level.unwrap_or(AcceptanceLevel::Auto);

        let level = if explicit_acceptance_can_disable(&explicit) {
            AcceptanceLevel::None
        } else if explicit_level == AcceptanceLevel::Auto {
            inferred.level
        } else {
            // MAX(explicit, inferred) by rank.
            let er = level_rank(explicit_level).unwrap_or(0);
            let ir = level_rank(inferred.level).unwrap_or(0);
            if er >= ir { explicit_level } else { inferred.level }
        };

        let base_evidence = if level == inferred.level {
            inferred.evidence.clone()
        } else {
            required_evidence_for_level(level)
        };
        let mut combined = base_evidence;
        if let Some(extra) = &explicit.evidence {
            combined.extend(extra.iter().copied());
        }
        let evidence = unique_evidence(&combined);

        let criteria_source: Vec<CriterionInput> = match &explicit.criteria {
            Some(criteria) if !criteria.is_empty() => criteria.clone(),
            _ => inferred.criteria.clone(),
        };
        let criteria = normalize_criteria(&criteria_source, &evidence);

        // `acceptance.ts:389` @v0.43.0: `explicit.review !== undefined ? explicit.review :
        // inferred.review` — and nothing more. v0.34.0 additionally downgraded an inference-
        // escalated `reviewed` gate to `required: false` (`acceptance.ts:288-290` @v0.34.0); that
        // rule existed only because inference could escalate the LEVEL to `reviewed`, which
        // v0.43.0 removed (see [`AcceptanceLevel`]), so the downgrade went with it.
        let review = if explicit.review.is_some() {
            explicit.review.clone()
        } else {
            inferred.review.clone()
        };

        ResolvedAcceptanceConfig {
            level,
            explicit: input.explicit.is_some(),
            inferred_reason: inferred.reasons,
            criteria,
            evidence,
            verify: explicit.verify.clone().unwrap_or_default(),
            review,
            stop_rules: explicit.stop_rules.clone().unwrap_or_default(),
            reason: explicit.reason.clone(),
        }
    }

    // --------------------------------------------------------------------------------------------
    // formatAcceptancePrompt (acceptance.ts:403-457)
    // --------------------------------------------------------------------------------------------

    /// The `acceptance-report` example object, rendered exactly as pi's
    /// `JSON.stringify(obj, null, 2)` produces it (acceptance.ts:333-344) — hand-written so the
    /// bytes (key order + 2-space indent) match pi's child-facing prompt verbatim.
    const ACCEPTANCE_REPORT_EXAMPLE: &str = r#"{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}"#;

    /// `acceptanceRequiresChildReport` (`acceptance.ts:403-405` @v0.43.0): whether this policy
    /// declares anything that can only be checked against the child's own structured report.
    ///
    /// It is exactly `criteria.length > 0 || evidence.length > 0`. Both of upstream's readers use
    /// it to avoid demanding a report the child was never told to write:
    /// [`format_acceptance_prompt`] suppresses the whole contract block for a `report_optional`
    /// caller whose policy requires none (`:409`), and [`evaluate_acceptance`] declines to reject a
    /// `report_optional` run for a merely-absent report (`:1251,1256`).
    #[must_use]
    pub fn acceptance_requires_child_report(acceptance: &ResolvedAcceptanceConfig) -> bool {
        !acceptance.criteria.is_empty() || !acceptance.evidence.is_empty()
    }

    /// `formatAcceptancePrompt` (`acceptance.ts:407-448` @v0.43.0).
    ///
    /// `report_optional` is upstream's `options.reportOptional` (`:407`): a caller that is not
    /// insisting on a structured report gets NO contract block at all when the policy declares
    /// neither criteria nor evidence (`:409`), which is the prompt half of the same flag
    /// [`EvaluateAcceptanceInput::report_optional`] carries into the gate.
    #[must_use]
    pub fn format_acceptance_prompt(
        acceptance: &ResolvedAcceptanceConfig,
        report_optional: bool,
    ) -> String {
        if acceptance.level == AcceptanceLevel::None {
            return String::new();
        }
        if report_optional && !acceptance_requires_child_report(acceptance) {
            return String::new();
        }
        let mut lines: Vec<String> = vec![
            String::new(),
            super::ACCEPTANCE_CONTRACT_HEADING.to_string(),
            format!("Acceptance level: {}", acceptance.level.as_str()),
            "Completion is not accepted from prose alone. End with a structured acceptance report."
                .to_string(),
            String::new(),
            "Criteria:".to_string(),
        ];
        if acceptance.criteria.is_empty() {
            lines.push("- Return the requested result.".to_string());
        } else {
            for criterion in &acceptance.criteria {
                lines.push(format!("- {}: {}", criterion.id, criterion.must));
            }
        }
        lines.push(String::new());
        let evidence_list = if acceptance.evidence.is_empty() {
            "none".to_string()
        } else {
            acceptance
                .evidence
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        lines.push(format!("Required evidence: {evidence_list}"));

        if !acceptance.verify.is_empty() {
            lines.push(String::new());
            lines.push("Runtime verification commands configured by parent:".to_string());
            for command in &acceptance.verify {
                lines.push(format!("- {}: {}", command.id, command.command));
            }
        }
        if let Some(ReviewSetting::Gate(gate)) = &acceptance.review {
            lines.push(String::new());
            let requiredness = if gate.required == Some(false) {
                "optional"
            } else {
                "required"
            };
            let by = gate
                .agent
                .as_deref()
                .map(|a| format!(" by {a}"))
                .unwrap_or_default();
            lines.push(format!("Review gate: {requiredness}{by}."));
            if let Some(focus) = &gate.focus {
                lines.push(format!("Review focus: {focus}"));
            }
        }
        if !acceptance.stop_rules.is_empty() {
            lines.push(String::new());
            lines.push("Stop rules:".to_string());
            for rule in &acceptance.stop_rules {
                lines.push(format!("- {rule}"));
            }
        }
        lines.push(String::new());
        lines.push(
            "Finish with a fenced JSON block tagged `acceptance-report` in this shape:".to_string(),
        );
        lines.push(
            "Use empty arrays when no items apply; array fields contain strings unless object entries are shown."
                .to_string(),
        );
        lines.push("```acceptance-report".to_string());
        lines.push(ACCEPTANCE_REPORT_EXAMPLE.to_string());
        lines.push("```".to_string());
        lines.join("\n")
    }

    // --------------------------------------------------------------------------------------------
    // Fenced-block scanning mirroring pi's regexes (acceptance.ts:459-482, 423-427, 494-515)
    // --------------------------------------------------------------------------------------------

    struct FenceMatch {
        /// Byte offset of the start of the whole match (INCLUDING an optional leading `\n`, for the
        /// trailing-fence variant — [`with_leading_newline`] records whether that newline was
        /// consumed).
        index: usize,
        /// Byte offset immediately after the whole match (including trailing `\s*` for the
        /// strip variant).
        end: usize,
        tag: String,
        body: String,
    }

    /// Locate every fenced block whose opening tag (case-insensitively) is one of `tags`, mirroring
    /// pi's `` /```${tag}\s*\n([\s\S]*?)```/gi ``. `with_leading_newline` extends `index` back over one
    /// optional leading `\n` and `with_trailing_ws` extends `end` over a trailing `\s*` run — the two
    /// extensions pi's `stripAcceptanceReport` regex adds over its `parseAcceptanceReport` one.
    fn fenced_matches(
        text: &str,
        tags: &[&str],
        with_leading_newline: bool,
        with_trailing_ws: bool,
    ) -> Vec<FenceMatch> {
        let bytes = text.as_bytes();
        let mut matches = Vec::new();
        let mut search_from = 0usize;
        while let Some(rel) = text.get(search_from..).and_then(|s| s.find("```")) {
            let fence_at = search_from + rel;
            let after_fence = fence_at + 3;
            // Read the tag token: characters up to the first whitespace/newline.
            let rest = text.get(after_fence..).unwrap_or("");
            let tag_end_rel = rest
                .find(|c: char| c.is_whitespace())
                .unwrap_or(rest.len());
            let tag = rest.get(..tag_end_rel).unwrap_or("").to_string();
            let tag_lower = tag.to_ascii_lowercase();
            if !tags.contains(&tag_lower.as_str()) {
                search_from = after_fence;
                continue;
            }
            // `\s*\n`: everything from tag end up to and including the first `\n` must be whitespace.
            let after_tag = after_fence + tag_end_rel;
            let after_tag_rest = text.get(after_tag..).unwrap_or("");
            let Some(nl_rel) = after_tag_rest.find('\n') else {
                search_from = after_fence;
                continue;
            };
            let inter = after_tag_rest.get(..nl_rel).unwrap_or("");
            if !inter.chars().all(char::is_whitespace) {
                search_from = after_fence;
                continue;
            }
            let body_start = after_tag + nl_rel + 1;
            // Body is non-greedy up to the next "```".
            let body_rest = text.get(body_start..).unwrap_or("");
            let Some(close_rel) = body_rest.find("```") else {
                search_from = after_fence;
                continue;
            };
            let close_at = body_start + close_rel;
            let body = body_rest.get(..close_rel).unwrap_or("").to_string();
            let mut end = close_at + 3;
            if with_trailing_ws {
                let tail = text.get(end..).unwrap_or("");
                let ws_len = tail
                    .char_indices()
                    .find(|(_, c)| !c.is_whitespace())
                    .map(|(i, _)| i)
                    .unwrap_or(tail.len());
                end += ws_len;
            }
            let mut index = fence_at;
            if with_leading_newline && fence_at > 0 && bytes.get(fence_at - 1) == Some(&b'\n') {
                index = fence_at - 1;
            }
            matches.push(FenceMatch {
                index,
                end,
                tag: tag_lower,
                body,
            });
            search_from = end.max(after_fence);
        }
        matches
    }

    /// `fencedBlocks(output, tag)` (acceptance.ts:423-427): every fenced block body (trimmed,
    /// non-empty) for the given tags.
    fn fenced_block_bodies(output: &str, tags: &[&str]) -> Vec<String> {
        fenced_matches(output, tags, false, false)
            .into_iter()
            .map(|m| m.body.trim().to_string())
            .filter(|body| !body.is_empty())
            .collect()
    }

    /// `extractBalancedJson` (acceptance.ts:459-482).
    fn extract_balanced_json(text: &str, start: usize) -> Option<String> {
        let mut depth = 0i64;
        let mut in_string = false;
        let mut escaped = false;
        let mut end: Option<usize> = Option::None;
        for (offset, ch) in text.get(start..).unwrap_or("").char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            if ch == '"' {
                in_string = true;
                continue;
            }
            if ch == '{' {
                depth += 1;
            }
            if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + offset + ch.len_utf8());
                    break;
                }
            }
        }
        end.and_then(|e| text.get(start..e)).map(str::to_string)
    }

    /// `parseReportJson` (acceptance.ts:646-658).
    fn parse_report_json(body: &str) -> Result<Value, String> {
        let trimmed = body.trim();
        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => Ok(value),
            Err(err) => {
                if let Some(json_start) = trimmed.find('{')
                    && json_start > 0
                        && let Some(json) = extract_balanced_json(trimmed, json_start) {
                            return serde_json::from_str::<Value>(&json)
                                .map_err(|e| e.to_string());
                        }
                Err(err.to_string())
            }
        }
    }

    // --------------------------------------------------------------------------------------------
    // G79: report normalization (acceptance.ts:484-628 @v0.43.0)
    //
    // v0.34.0 had only `unwrapAcceptanceReport` + `validationPathLabelForWrapper`: two wrapper keys,
    // no aliases, no coercions, and no errors for anything unexpected. v0.43.0 replaced both with a
    // single normalization pass that runs INSIDE `validateAcceptanceReport`, so every entry point
    // (explicit fence, generic JSON fence, `ACCEPTANCE_REPORT:` marker, caller-supplied report)
    // gets the same treatment.
    // --------------------------------------------------------------------------------------------

    /// `ACCEPTANCE_REPORT_WRAPPERS` (acceptance.ts:484) — four spellings, up from v0.34.0's two.
    const ACCEPTANCE_REPORT_WRAPPERS: &[&str] = &[
        "acceptance",
        "acceptance-report",
        "acceptance_report",
        "acceptanceReport",
    ];

    /// `ACCEPTANCE_REPORT_FIELDS` (acceptance.ts:486-508) — `(wire key, canonical key)`. Every
    /// camelCase field also answers to its snake_case spelling, and `notes` is canonical on its own.
    const ACCEPTANCE_REPORT_FIELDS: &[(&str, &str)] = &[
        ("criteriaSatisfied", "criteriaSatisfied"),
        ("criteria_satisfied", "criteriaSatisfied"),
        ("changedFiles", "changedFiles"),
        ("changed_files", "changedFiles"),
        ("testsAddedOrUpdated", "testsAddedOrUpdated"),
        ("tests_added_or_updated", "testsAddedOrUpdated"),
        ("commandsRun", "commandsRun"),
        ("commands_run", "commandsRun"),
        ("validationOutput", "validationOutput"),
        ("validation_output", "validationOutput"),
        ("residualRisks", "residualRisks"),
        ("residual_risks", "residualRisks"),
        ("noStagedFiles", "noStagedFiles"),
        ("no_staged_files", "noStagedFiles"),
        ("diffSummary", "diffSummary"),
        ("diff_summary", "diffSummary"),
        ("reviewFindings", "reviewFindings"),
        ("review_findings", "reviewFindings"),
        ("manualNotes", "manualNotes"),
        ("manual_notes", "manualNotes"),
        ("notes", "notes"),
    ];

    /// `CRITERION_REPORT_FIELDS` (acceptance.ts:510).
    const CRITERION_REPORT_FIELDS: &[&str] = &["id", "status", "evidence"];
    /// `COMMAND_REPORT_FIELDS` (acceptance.ts:511).
    const COMMAND_REPORT_FIELDS: &[&str] = &["command", "result", "summary"];

    /// `normalizedToken` (acceptance.ts:513-515):
    /// `value.trim().toLowerCase().replace(/[\s_]+/g, "-").replace(/-+/g, "-")`.
    ///
    /// Both replacements are reproduced literally, in order, because they are not the same as one
    /// combined `[\s_-]+ -> "-"` pass at the edges: `trim()` has already removed surrounding
    /// whitespace, so a leading `_` or `-` survives as a leading `-` rather than being dropped.
    fn normalized_token(value: &str) -> String {
        let lowered = value.trim().to_lowercase();
        // `/[\s_]+/g -> "-"`
        let mut collapsed = String::with_capacity(lowered.len());
        let mut in_run = false;
        for ch in lowered.chars() {
            if ch.is_whitespace() || ch == '_' {
                if !in_run {
                    collapsed.push('-');
                    in_run = true;
                }
            } else {
                in_run = false;
                collapsed.push(ch);
            }
        }
        // `/-+/g -> "-"`
        let mut out = String::with_capacity(collapsed.len());
        let mut prev_dash = false;
        for ch in collapsed.chars() {
            if ch == '-' {
                if !prev_dash {
                    out.push('-');
                    prev_dash = true;
                }
            } else {
                prev_dash = false;
                out.push(ch);
            }
        }
        out
    }

    /// `normalizeCriterionStatus` (acceptance.ts:517-524) — the alias table a model actually emits
    /// (`"Done"`, `"PASSED"`, `"n/a"`-style tokens) folded onto the three canonical statuses. A
    /// value that matches nothing is returned UNCHANGED so `validateAcceptanceReport` still reports
    /// it with its original text.
    fn normalize_criterion_status(value: &Value) -> Value {
        let Some(text) = value.as_str() else {
            return value.clone();
        };
        let token = normalized_token(text);
        match token.as_str() {
            "satisfied" | "met" | "complete" | "completed" | "done" | "pass" | "passed"
            | "success" | "succeeded" => Value::String("satisfied".to_string()),
            "not-satisfied" | "not-met" | "unmet" | "incomplete" | "fail" | "failed" => {
                Value::String("not-satisfied".to_string())
            }
            "not-applicable" | "n-a" | "na" | "skip" | "skipped" => {
                Value::String("not-applicable".to_string())
            }
            _ => value.clone(),
        }
    }

    /// `normalizeCommandResult` (acceptance.ts:526-533).
    fn normalize_command_result(value: &Value) -> Value {
        let Some(text) = value.as_str() else {
            return value.clone();
        };
        let token = normalized_token(text);
        match token.as_str() {
            "passed" | "pass" | "success" | "successful" | "succeeded" | "ok" => {
                Value::String("passed".to_string())
            }
            "failed" | "fail" | "failure" | "error" => Value::String("failed".to_string()),
            "not-run" | "not-executed" | "skip" | "skipped" => {
                Value::String("not-run".to_string())
            }
            _ => value.clone(),
        }
    }

    /// `normalizeCriterionReport` (acceptance.ts:535-550).
    fn normalize_criterion_report(value: &Value, path_label: &str, errors: &mut Vec<String>) -> Value {
        let Value::Object(map) = value else {
            return value.clone();
        };
        let mut out = serde_json::Map::new();
        for (key, field) in map {
            if !CRITERION_REPORT_FIELDS.contains(&key.as_str()) {
                errors.push(format!(
                    "{path_label}.{key}: unsupported acceptance criterion field"
                ));
                continue;
            }
            let normalized = match key.as_str() {
                "id" => match field.as_str() {
                    Some(text) => Value::String(normalized_token(text)),
                    Option::None => field.clone(),
                },
                "status" => normalize_criterion_status(field),
                _ => field.clone(),
            };
            out.insert(key.clone(), normalized);
        }
        Value::Object(out)
    }

    /// `normalizeCommandReport` (acceptance.ts:552-563).
    fn normalize_command_report(value: &Value, path_label: &str, errors: &mut Vec<String>) -> Value {
        let Value::Object(map) = value else {
            return value.clone();
        };
        let mut out = serde_json::Map::new();
        for (key, field) in map {
            if !COMMAND_REPORT_FIELDS.contains(&key.as_str()) {
                errors.push(format!(
                    "{path_label}.{key}: unsupported acceptance command field"
                ));
                continue;
            }
            let normalized = if key == "result" {
                normalize_command_result(field)
            } else {
                field.clone()
            };
            out.insert(key.clone(), normalized);
        }
        Value::Object(out)
    }

    /// The outcome of [`normalize_acceptance_report_value`].
    struct NormalizedReportValue {
        value: Value,
        path_label: String,
        errors: Vec<String>,
    }

    /// `normalizeAcceptanceReportValue` (acceptance.ts:565-628): unwrap an acceptance wrapper key,
    /// fold every field alias onto its canonical name, and coerce the shapes a model reliably gets
    /// wrong — a lone object where an array belongs, a bare string where a `string[]` belongs, and
    /// `"true"`/`"false"` where `noStagedFiles` wants a boolean.
    ///
    /// **[CYRUP-DELTA]** `serde_json::Map` is a `BTreeMap` here (the `preserve_order` feature is
    /// off workspace-wide), so key iteration is alphabetical rather than insertion-ordered. That
    /// changes only two things against upstream and neither is a behavioural gate: which of several
    /// simultaneously-present wrapper keys is picked as `wrapperKey` (upstream picks the first
    /// authored, this picks the alphabetically first — and either way the ambiguity itself is
    /// reported), and the ORDER of the accumulated error strings.
    fn normalize_acceptance_report_value(value: &Value, path_label: &str) -> NormalizedReportValue {
        let mut errors: Vec<String> = Vec::new();
        let mut report_value = value.clone();
        let mut report_path = path_label.to_string();

        if let Value::Object(map) = &report_value {
            let wrapper_keys: Vec<&String> = map
                .keys()
                .filter(|key| ACCEPTANCE_REPORT_WRAPPERS.contains(&key.as_str()))
                .collect();
            if let Some(wrapper_key) = wrapper_keys.first().map(|key| (*key).clone()) {
                if wrapper_keys.len() > 1 {
                    let label = if path_label.is_empty() {
                        "acceptance-report"
                    } else {
                        path_label
                    };
                    errors.push(format!(
                        "{label}: multiple acceptance report wrappers are ambiguous"
                    ));
                }
                for key in map.keys() {
                    if key != &wrapper_key {
                        errors.push(format!(
                            "{}: unsupported alongside acceptance report wrapper '{wrapper_key}'",
                            path_for(path_label, key)
                        ));
                    }
                }
                report_path = path_for(path_label, &wrapper_key);
                report_value = map.get(&wrapper_key).cloned().unwrap_or(Value::Null);
            }
        }

        let Value::Object(map) = &report_value else {
            return NormalizedReportValue {
                value: report_value,
                path_label: report_path,
                errors,
            };
        };

        let mut normalized = serde_json::Map::new();
        for (key, field) in map {
            let Some((_, canonical)) = ACCEPTANCE_REPORT_FIELDS
                .iter()
                .find(|(wire, _)| *wire == key.as_str())
            else {
                errors.push(format!(
                    "{}: unsupported acceptance report field",
                    path_for(&report_path, key)
                ));
                continue;
            };
            if normalized.contains_key(*canonical) {
                errors.push(format!(
                    "{}: duplicates normalized field '{canonical}'",
                    path_for(&report_path, key)
                ));
                continue;
            }
            let field_path = path_for(&report_path, canonical);
            let normalized_field = match *canonical {
                "criteriaSatisfied" | "commandsRun" => {
                    // A lone object is read as a one-element array (`acceptance.ts:598,605`).
                    let items: Option<Vec<Value>> = match field {
                        Value::Array(items) => Some(items.clone()),
                        Value::Object(_) => Some(vec![field.clone()]),
                        _ => Option::None,
                    };
                    match items {
                        Some(items) => Value::Array(
                            items
                                .iter()
                                .enumerate()
                                .map(|(index, item)| {
                                    let item_path = format!("{field_path}[{index}]");
                                    if *canonical == "criteriaSatisfied" {
                                        normalize_criterion_report(item, &item_path, &mut errors)
                                    } else {
                                        normalize_command_report(item, &item_path, &mut errors)
                                    }
                                })
                                .collect(),
                        ),
                        Option::None => field.clone(),
                    }
                }
                "changedFiles" | "testsAddedOrUpdated" | "validationOutput" | "residualRisks"
                | "reviewFindings" => match field {
                    Value::String(_) => Value::Array(vec![field.clone()]),
                    _ => field.clone(),
                },
                "noStagedFiles" => match field.as_str().map(|s| s.trim().to_lowercase()) {
                    Some(token) if token == "true" => Value::Bool(true),
                    Some(token) if token == "false" => Value::Bool(false),
                    _ => field.clone(),
                },
                _ => field.clone(),
            };
            normalized.insert((*canonical).to_string(), normalized_field);
        }

        NormalizedReportValue {
            value: Value::Object(normalized),
            path_label: report_path,
            errors,
        }
    }

    // --------------------------------------------------------------------------------------------
    // validateAcceptanceReport (acceptance.ts:517-620) — exact error-message parity
    // --------------------------------------------------------------------------------------------

    fn describe_validation_value(value: Option<&Value>) -> String {
        match value {
            Option::None => "missing".to_string(),
            Some(Value::Null) => "null".to_string(),
            Some(Value::Array(_)) => "array".to_string(),
            Some(Value::Object(_)) => "object".to_string(),
            Some(Value::String(s)) => {
                let short: String = if s.chars().count() > 80 {
                    let head: String = s.chars().take(77).collect();
                    format!("{head}...")
                } else {
                    s.clone()
                };
                serde_json::to_string(&Value::String(short))
                    .unwrap_or_else(|_| format!("\"{s}\""))
            }
            Some(Value::Number(n)) => format!("number {n}"),
            Some(Value::Bool(b)) => format!("boolean {b}"),
        }
    }

    fn push_type_error(errors: &mut Vec<String>, path: &str, expected: &str, value: Option<&Value>) {
        errors.push(format!(
            "{path}: expected {expected}; got {}",
            describe_validation_value(value)
        ));
    }

    fn path_for(base: &str, segment: &str) -> String {
        if base.is_empty() {
            segment.to_string()
        } else {
            format!("{base}.{segment}")
        }
    }

    fn validate_string_array_field(errors: &mut Vec<String>, value: Option<&Value>, path: &str) {
        let Some(Value::Array(items)) = value else {
            push_type_error(errors, path, "string[]", value);
            return;
        };
        for (index, item) in items.iter().enumerate() {
            // `acceptance.ts:827` @v0.43.0 — a blank entry is no longer accepted as evidence
            // (v0.34.0 only required `typeof item === "string"`).
            if !item.as_str().is_some_and(|s| !s.trim().is_empty()) {
                push_type_error(
                    errors,
                    &format!("{path}[{index}]"),
                    "non-empty string",
                    Some(item),
                );
            }
        }
    }

    /// `validateAcceptanceReport` (acceptance.ts:831-909). Returns `(report, errors)`; a present
    /// `report` requires no errors AND at least one recognized report field.
    fn validate_acceptance_report(
        value: &Value,
        path_label: &str,
    ) -> (Option<AcceptanceReport>, Vec<String>) {
        // `acceptance.ts:831-835` @v0.43.0 — normalization runs FIRST and its errors seed the list,
        // so an unsupported alias or a duplicated field is reported alongside the type errors
        // rather than silently dropping the whole report.
        let normalized = normalize_acceptance_report_value(value, path_label);
        let value = &normalized.value;
        let path_label = normalized.path_label.as_str();
        let mut errors: Vec<String> = normalized.errors;
        let Value::Object(map) = value else {
            let label = if path_label.is_empty() {
                "acceptance-report"
            } else {
                path_label
            };
            push_type_error(&mut errors, label, "object", Some(value));
            return (Option::None, errors);
        };

        // criteriaSatisfied
        if let Some(criteria) = map.get("criteriaSatisfied") {
            let cpath = path_for(path_label, "criteriaSatisfied");
            if let Value::Array(items) = criteria {
                // `acceptance.ts:845,855-858`: ids are compared AFTER `normalizedToken`, so
                // `"C 1"` and `"c_1"` are the same criterion and the second one is a duplicate.
                let mut criterion_ids: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for (index, item) in items.iter().enumerate() {
                    let ipath = format!("{cpath}[{index}]");
                    let Value::Object(obj) = item else {
                        push_type_error(&mut errors, &ipath, "object", Some(item));
                        continue;
                    };
                    match obj.get("id") {
                        Some(id) if !id.is_string() => {
                            push_type_error(&mut errors, &format!("{ipath}.id"), "string", Some(id));
                        }
                        Some(Value::String(id))
                            if !id.is_empty() && !criterion_ids.insert(id.as_str()) =>
                        {
                            errors.push(format!(
                                "{ipath}.id: duplicate normalized criterion id '{id}'"
                            ));
                        }
                        _ => {}
                    }
                    let status = obj.get("status").and_then(Value::as_str);
                    if !matches!(status, Some("satisfied") | Some("not-satisfied") | Some("not-applicable"))
                    {
                        push_type_error(
                            &mut errors,
                            &format!("{ipath}.status"),
                            "one of \"satisfied\", \"not-satisfied\", \"not-applicable\"",
                            obj.get("status"),
                        );
                    }
                    let evidence_ok = obj
                        .get("evidence")
                        .and_then(Value::as_str)
                        .is_some_and(|e| !e.trim().is_empty());
                    if !evidence_ok {
                        push_type_error(
                            &mut errors,
                            &format!("{ipath}.evidence"),
                            "non-empty string",
                            obj.get("evidence"),
                        );
                    }
                }
            } else {
                push_type_error(&mut errors, &cpath, "array", Some(criteria));
            }
        }

        if map.contains_key("changedFiles") {
            validate_string_array_field(
                &mut errors,
                map.get("changedFiles"),
                &path_for(path_label, "changedFiles"),
            );
        }
        if map.contains_key("testsAddedOrUpdated") {
            validate_string_array_field(
                &mut errors,
                map.get("testsAddedOrUpdated"),
                &path_for(path_label, "testsAddedOrUpdated"),
            );
        }
        if let Some(commands) = map.get("commandsRun") {
            let cpath = path_for(path_label, "commandsRun");
            if let Value::Array(items) = commands {
                for (index, item) in items.iter().enumerate() {
                    let ipath = format!("{cpath}[{index}]");
                    let Value::Object(obj) = item else {
                        push_type_error(&mut errors, &ipath, "object", Some(item));
                        continue;
                    };
                    let command_ok = obj
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|c| !c.trim().is_empty());
                    if !command_ok {
                        push_type_error(
                            &mut errors,
                            &format!("{ipath}.command"),
                            "non-empty string",
                            obj.get("command"),
                        );
                    }
                    let result = obj.get("result").and_then(Value::as_str);
                    if !matches!(result, Some("passed") | Some("failed") | Some("not-run")) {
                        push_type_error(
                            &mut errors,
                            &format!("{ipath}.result"),
                            "one of \"passed\", \"failed\", \"not-run\"",
                            obj.get("result"),
                        );
                    }
                    // `acceptance.ts:883` @v0.43.0 — non-empty (v0.34.0 accepted `""`).
                    if !obj
                        .get("summary")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.trim().is_empty())
                    {
                        push_type_error(
                            &mut errors,
                            &format!("{ipath}.summary"),
                            "non-empty string",
                            obj.get("summary"),
                        );
                    }
                }
            } else {
                push_type_error(&mut errors, &cpath, "array", Some(commands));
            }
        }
        if map.contains_key("validationOutput") {
            validate_string_array_field(
                &mut errors,
                map.get("validationOutput"),
                &path_for(path_label, "validationOutput"),
            );
        }
        if map.contains_key("residualRisks") {
            validate_string_array_field(
                &mut errors,
                map.get("residualRisks"),
                &path_for(path_label, "residualRisks"),
            );
        }
        if let Some(no_staged) = map.get("noStagedFiles")
            && !no_staged.is_boolean() {
                push_type_error(
                    &mut errors,
                    &path_for(path_label, "noStagedFiles"),
                    "boolean",
                    Some(no_staged),
                );
            }
        // `acceptance.ts:890` @v0.43.0 — non-empty (v0.34.0 accepted `""`).
        if let Some(diff) = map.get("diffSummary")
            && !diff.as_str().is_some_and(|s| !s.trim().is_empty()) {
                push_type_error(
                    &mut errors,
                    &path_for(path_label, "diffSummary"),
                    "non-empty string",
                    Some(diff),
                );
            }
        if map.contains_key("reviewFindings") {
            validate_string_array_field(
                &mut errors,
                map.get("reviewFindings"),
                &path_for(path_label, "reviewFindings"),
            );
        }
        if let Some(notes) = map.get("manualNotes")
            && !notes.is_string() {
                push_type_error(
                    &mut errors,
                    &path_for(path_label, "manualNotes"),
                    "string",
                    Some(notes),
                );
            }
        if let Some(notes) = map.get("notes")
            && !notes.is_string() {
                push_type_error(
                    &mut errors,
                    &path_for(path_label, "notes"),
                    "string",
                    Some(notes),
                );
            }

        if !errors.is_empty() {
            return (Option::None, errors);
        }
        let has_field = [
            "criteriaSatisfied",
            "changedFiles",
            "testsAddedOrUpdated",
            "commandsRun",
            "validationOutput",
            "residualRisks",
            "noStagedFiles",
            "diffSummary",
            "manualNotes",
            "notes",
            "reviewFindings",
        ]
        .iter()
        .any(|key| map.contains_key(*key));
        if !has_field {
            let label = if path_label.is_empty() {
                "acceptance-report"
            } else {
                path_label
            };
            return (
                Option::None,
                vec![format!("{label}: expected at least one acceptance report field")],
            );
        }
        // Validation passed: deserialize the known fields into the typed report (unknown fields are
        // ignored, matching pi's tolerant read-back).
        match serde_json::from_value::<AcceptanceReport>(value.clone()) {
            Ok(report) => (Some(report), errors),
            Err(err) => (Option::None, vec![err.to_string()]),
        }
    }

    /// `hasGenericAcceptanceReportSignal` (acceptance.ts:630-644 @v0.43.0) — plain KEY PRESENCE on
    /// the NORMALIZED value.
    ///
    /// v0.34.0 additionally type-checked each companion (`isStringArray(record.changedFiles)`, …),
    /// which meant a `json`-fenced report whose `changedFiles` was a bare string — a shape v0.43.0's
    /// normalizer now repairs — read as "not a report at all" and was silently left in the
    /// delivered output. Presence alone is the signal; the shape is then `validateAcceptanceReport`'s
    /// business and its errors are surfaced rather than swallowed.
    fn has_generic_acceptance_report_signal(value: &Value) -> bool {
        let Value::Object(map) = value else {
            return false;
        };
        if !map.contains_key("criteriaSatisfied") {
            return false;
        }
        [
            "changedFiles",
            "testsAddedOrUpdated",
            "commandsRun",
            "validationOutput",
            "residualRisks",
            "noStagedFiles",
            "diffSummary",
            "reviewFindings",
            "manualNotes",
        ]
        .iter()
        .any(|key| map.contains_key(*key))
    }

    // --------------------------------------------------------------------------------------------
    // parseAcceptanceReport / stripAcceptanceReport (acceptance.ts:437-515)
    // --------------------------------------------------------------------------------------------

    /// `parseAcceptanceReport` result (acceptance.ts:701).
    #[derive(Debug, Clone, PartialEq, Default)]
    pub struct ParsedAcceptanceReport {
        pub report: Option<AcceptanceReport>,
        pub error: Option<String>,
    }

    /// `ACCEPTANCE_REPORT_NOT_FOUND` (acceptance.ts:699) — the ONE error string that means "there
    /// was simply no report here", as opposed to "there was one and it was broken". The source
    /// fallback in [`parse_acceptance_report_sources`] switches on exactly this value, so it is a
    /// named constant rather than a repeated literal.
    pub const ACCEPTANCE_REPORT_NOT_FOUND: &str = "Structured acceptance report not found.";

    /// The tag alternation `` ```acceptance[-_]report `` (acceptance.ts:702-703) — v0.43.0 accepts
    /// the underscore spelling everywhere the hyphenated one is accepted.
    const ACCEPTANCE_REPORT_FENCE_TAGS: &[&str] = &["acceptance-report", "acceptance_report"];

    /// `parseAcceptanceReportBody` (acceptance.ts:666-668).
    fn parse_acceptance_report_body(body: &str) -> Result<(Option<AcceptanceReport>, Vec<String>), String> {
        let parsed = parse_report_json(body)?;
        Ok(validate_acceptance_report(&parsed, ""))
    }

    /// `parseUnterminatedAcceptanceReportFence` (acceptance.ts:670-683) — recovery for the single
    /// most common malformed emission: the model opened ```` ```acceptance-report ```` and then ran
    /// out of turn without ever closing the fence. Everything after the opener is parsed as the
    /// body. Only attempted when NO closing ``` follows the opener at all, so a well-formed run is
    /// never re-parsed by this path.
    ///
    /// Returns `(report, error)`; `(None, None)` means "this recovery does not apply".
    fn parse_unterminated_acceptance_report_fence(
        output: &str,
    ) -> (Option<AcceptanceReport>, Option<String>) {
        let Some((body_start, _)) = find_acceptance_report_fence_opener(output) else {
            return (Option::None, Option::None);
        };
        if output.get(body_start..).is_some_and(|rest| rest.contains("```")) {
            return (Option::None, Option::None);
        }
        let body = output.get(body_start..).unwrap_or("").trim();
        match serde_json::from_str::<Value>(body) {
            Ok(parsed) => {
                let (report, errors) = validate_acceptance_report(&parsed, "");
                match report {
                    Some(report) => (Some(report), Option::None),
                    Option::None => (
                        Option::None,
                        Some(format!(
                            "Failed to parse acceptance-report: Invalid acceptance-report: {}",
                            errors.join("; ")
                        )),
                    ),
                }
            }
            Err(err) => (
                Option::None,
                Some(format!("Failed to parse acceptance-report: {err}")),
            ),
        }
    }

    /// `/```acceptance[-_]report\b/i.test(output)` (acceptance.ts:702) — pure TAG PRESENCE, with a
    /// `\b` after the tag and NO requirement that a newline follow it.
    ///
    /// This is deliberately NOT [`find_acceptance_report_fence_opener`], whose upstream twin
    /// (`parseUnterminatedAcceptanceReportFence`, acceptance.ts:671) really does anchor on
    /// `[^\n]*\n` because it needs the offset where the fence BODY starts. `parseAcceptanceReport`
    /// only needs to know a fence was OPENED. Conflating the two loses exactly the case where a
    /// model was cut off mid-opener (`"…\n```acceptance-report"` with nothing after it): the run
    /// then reports [`ACCEPTANCE_REPORT_NOT_FOUND`] instead of the fence defect, and because that
    /// constant is the one value [`parse_acceptance_report_sources`] and
    /// [`super::select_acceptance_report_source`] branch on to decide "genuinely absent, fall
    /// through to the other source", a truncated report in a `file-only` artifact papers itself
    /// over with the assistant text — the precise failure this source-selection rule exists to
    /// prevent.
    fn has_acceptance_report_fence_tag(output: &str) -> bool {
        let lowered = output.to_ascii_lowercase();
        let mut from = 0usize;
        while let Some(rel) = lowered.get(from..).and_then(|s| s.find("```")) {
            let fence_at = from + rel;
            let after_fence = fence_at + 3;
            let rest = lowered.get(after_fence..).unwrap_or("");
            if let Some(tag) = ACCEPTANCE_REPORT_FENCE_TAGS
                .iter()
                .find(|tag| rest.starts_with(**tag))
            {
                let after_tag = after_fence + tag.len();
                // `\b`: the tag must not run straight into another word character.
                let boundary_ok = lowered
                    .get(after_tag..)
                    .and_then(|s| s.chars().next())
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_');
                if boundary_ok {
                    return true;
                }
            }
            from = after_fence;
        }
        false
    }

    /// `/```acceptance[-_]report\b[^\n]*\n/gi.exec(output)` (acceptance.ts:671-673): the byte offset
    /// just past the opener's newline, plus the offset of the opener itself.
    fn find_acceptance_report_fence_opener(output: &str) -> Option<(usize, usize)> {
        let lowered = output.to_ascii_lowercase();
        let mut from = 0usize;
        while let Some(rel) = lowered.get(from..).and_then(|s| s.find("```")) {
            let fence_at = from + rel;
            let after_fence = fence_at + 3;
            let rest = lowered.get(after_fence..).unwrap_or("");
            let tag = ACCEPTANCE_REPORT_FENCE_TAGS
                .iter()
                .find(|tag| rest.starts_with(**tag));
            if let Some(tag) = tag {
                let after_tag = after_fence + tag.len();
                // `\b`: the tag must not run straight into another word character.
                let boundary_ok = lowered
                    .get(after_tag..)
                    .and_then(|s| s.chars().next())
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_');
                if boundary_ok
                    && let Some(nl_rel) = lowered.get(after_tag..).and_then(|s| s.find('\n'))
                {
                    return Some((after_tag + nl_rel + 1, fence_at));
                }
            }
            from = after_fence;
        }
        Option::None
    }

    /// `parseGenericJsonAcceptanceReportBody` (acceptance.ts:685-697).
    ///
    /// Returns `(report, error)`. v0.34.0 returned only `Option<AcceptanceReport>` and swallowed
    /// every validation failure, so a `json`-fenced block that was unmistakably a report but had one
    /// bad field was treated as unrelated prose. v0.43.0 surfaces those errors — but ONLY when the
    /// value still carries the `criteriaSatisfied` marker, so genuinely unrelated JSON stays quiet.
    fn parse_generic_json_acceptance_report_body(
        body: &str,
    ) -> Result<(Option<AcceptanceReport>, Option<String>), String> {
        let parsed = parse_report_json(body)?;
        let normalized = normalize_acceptance_report_value(&parsed, "");
        let has_criteria_marker = matches!(
            &normalized.value,
            Value::Object(map) if map.contains_key("criteriaSatisfied")
        );
        if !has_generic_acceptance_report_signal(&normalized.value)
            && !(has_criteria_marker && !normalized.errors.is_empty())
        {
            return Ok((Option::None, Option::None));
        }
        let (report, errors) = validate_acceptance_report(&parsed, "");
        Ok(match report {
            Some(report) => (Some(report), Option::None),
            Option::None => (
                Option::None,
                Some(format!("Invalid acceptance-report: {}", errors.join("; "))),
            ),
        })
    }

    /// `parseAcceptanceReport` (acceptance.ts:701-751).
    #[must_use]
    pub fn parse_acceptance_report(output: &str) -> ParsedAcceptanceReport {
        // acceptance.ts:702 tests TAG PRESENCE only — see [`has_acceptance_report_fence_tag`] for
        // why this must not be `find_acceptance_report_fence_opener(...).is_some()`.
        let explicit_fence_present = has_acceptance_report_fence_tag(output);
        let fenced = fenced_block_bodies(output, ACCEPTANCE_REPORT_FENCE_TAGS);
        let mut parse_errors: Vec<String> = Vec::new();
        for body in &fenced {
            match parse_acceptance_report_body(body) {
                Ok((Some(report), _)) => {
                    return ParsedAcceptanceReport {
                        report: Some(report),
                        error: Option::None,
                    };
                }
                Ok((Option::None, errors)) => {
                    parse_errors.push(format!("Invalid acceptance-report: {}", errors.join("; ")));
                }
                Err(message) => parse_errors.push(message),
            }
        }
        if !parse_errors.is_empty() {
            return ParsedAcceptanceReport {
                report: Option::None,
                error: Some(format!(
                    "Failed to parse acceptance-report: {}",
                    parse_errors.join("; ")
                )),
            };
        }
        // `acceptance.ts:715-719`: an OPENED acceptance-report fence that produced no parseable
        // body never falls through to the generic-JSON or marker paths — it is a defect of THIS
        // report, so it is either recovered or reported.
        if explicit_fence_present {
            let (report, error) = parse_unterminated_acceptance_report_fence(output);
            if report.is_some() || error.is_some() {
                return ParsedAcceptanceReport { report, error };
            }
            return ParsedAcceptanceReport {
                report: Option::None,
                error: Some(
                    "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
                        .to_string(),
                ),
            };
        }
        for body in fenced_block_bodies(output, &["json", "jsonc", "json5"]) {
            match parse_generic_json_acceptance_report_body(&body) {
                Ok((Some(report), _)) => {
                    return ParsedAcceptanceReport {
                        report: Some(report),
                        error: Option::None,
                    };
                }
                Ok((Option::None, Some(error))) => {
                    return ParsedAcceptanceReport {
                        report: Option::None,
                        error: Some(format!("Failed to parse acceptance-report: {error}")),
                    };
                }
                // Unrelated JSON, or malformed JSON that is not report-shaped: ignored, exactly as
                // upstream's bare `catch {}` does (`acceptance.ts:725-728`).
                Ok((Option::None, Option::None)) | Err(_) => {}
            }
        }
        // ACCEPTANCE_REPORT: marker (acceptance.ts:730-749). v0.43.0 gives the two "the marker is
        // there but the object is not" cases their own messages instead of silently falling through
        // to `ACCEPTANCE_REPORT_NOT_FOUND`.
        if let Some(marker_index) = find_acceptance_report_marker(output) {
            let Some(json_start) = output
                .get(marker_index..)
                .and_then(|s| s.find('{'))
                .map(|r| marker_index + r)
            else {
                return ParsedAcceptanceReport {
                    report: Option::None,
                    error: Some(
                        "Failed to parse acceptance-report: Expected a JSON object after ACCEPTANCE_REPORT:."
                            .to_string(),
                    ),
                };
            };
            let Some(json) = extract_balanced_json(output, json_start) else {
                return ParsedAcceptanceReport {
                    report: Option::None,
                    error: Some(
                        "Failed to parse acceptance-report: Unterminated JSON object after ACCEPTANCE_REPORT:."
                            .to_string(),
                    ),
                };
            };
            return match serde_json::from_str::<Value>(&json) {
                Ok(parsed) => {
                    let (validated, errors) = validate_acceptance_report(&parsed, "");
                    match validated {
                        Some(report) => ParsedAcceptanceReport {
                            report: Some(report),
                            error: Option::None,
                        },
                        Option::None => ParsedAcceptanceReport {
                            report: Option::None,
                            error: Some(format!(
                                "Failed to parse acceptance-report: Invalid acceptance-report: {}",
                                errors.join("; ")
                            )),
                        },
                    }
                }
                Err(err) => ParsedAcceptanceReport {
                    report: Option::None,
                    error: Some(format!("Failed to parse acceptance-report: {err}")),
                },
            };
        }
        ParsedAcceptanceReport {
            report: Option::None,
            error: Some(ACCEPTANCE_REPORT_NOT_FOUND.to_string()),
        }
    }

    /// `parseAcceptanceReportSources` (acceptance.ts:753-772) — search BOTH the assistant output and
    /// the child's configured output file, in the order `authoritative` dictates.
    ///
    /// The load-bearing rule is the fallthrough condition: only a genuinely ABSENT report
    /// ([`ACCEPTANCE_REPORT_NOT_FOUND`]) in the primary source falls through to the secondary. A
    /// MALFORMED report in the primary source is a defect to surface, not a miss to paper over.
    #[must_use]
    pub fn parse_acceptance_report_sources(
        output: &str,
        file_output: Option<&super::AcceptanceFileOutput<'_>>,
    ) -> ParsedAcceptanceReport {
        let from_text = || parse_acceptance_report(output);
        let from_file = || match file_output {
            Option::None => ParsedAcceptanceReport {
                report: Option::None,
                error: Some(ACCEPTANCE_REPORT_NOT_FOUND.to_string()),
            },
            Some(file) => {
                let parsed = parse_acceptance_report(file.content);
                if parsed.report.is_some()
                    || parsed.error.as_deref() == Some(ACCEPTANCE_REPORT_NOT_FOUND)
                {
                    parsed
                } else {
                    ParsedAcceptanceReport {
                        report: Option::None,
                        error: Some(format!(
                            "{} (in configured output {})",
                            parsed.error.unwrap_or_default(),
                            file.path.display()
                        )),
                    }
                }
            }
        };
        let authoritative = file_output.is_some_and(|file| file.authoritative);
        let first = if authoritative { from_file() } else { from_text() };
        if first.report.is_some() || first.error.as_deref() != Some(ACCEPTANCE_REPORT_NOT_FOUND) {
            return first;
        }
        if authoritative { from_text() } else { from_file() }
    }

    /// Case-insensitive `/ACCEPTANCE_REPORT\s*:/i` locator (acceptance.ts:473).
    fn find_acceptance_report_marker(output: &str) -> Option<usize> {
        let upper = output.to_ascii_uppercase();
        let mut from = 0usize;
        while let Some(rel) = upper.get(from..).and_then(|s| s.find("ACCEPTANCE_REPORT")) {
            let at = from + rel;
            let after = at + "ACCEPTANCE_REPORT".len();
            let rest = upper.get(after..).unwrap_or("");
            let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r']);
            if trimmed.starts_with(':') {
                return Some(at);
            }
            from = at + 1;
        }
        Option::None
    }

    /// `stripAcceptanceReport` (acceptance.ts:774-795). Removes a trailing `acceptance-report` /
    /// generic-JSON acceptance-report fence (and a trailing `ACCEPTANCE_REPORT: {...}` marker) from
    /// the DELIVERED output, so a caller sees the human answer, never the machine report JSON.
    #[must_use]
    pub fn strip_acceptance_report(output: &str) -> String {
        // The trailing-fence variant (`\n?```(acceptance[-_]report|json|jsonc|json5)\s*\n([\s\S]*?)```\s*`,
        // acceptance.ts:775) — five tags at v0.43.0, which added the underscore spelling.
        let tags = [
            "acceptance-report",
            "acceptance_report",
            "json",
            "jsonc",
            "json5",
        ];
        let matches = fenced_matches(output, &tags, true, true);
        // The LAST match with only whitespace after it is the trailing fence (acceptance.ts:777-782).
        let trailing = matches.into_iter().rev().find(|m| {
            output
                .get(m.end..)
                .is_none_or(|tail| tail.trim().is_empty())
        });
        if let Some(fence) = trailing {
            if ACCEPTANCE_REPORT_FENCE_TAGS.contains(&fence.tag.as_str()) {
                return output.get(..fence.index).unwrap_or("").trim_end().to_string();
            }
            if matches!(
                parse_generic_json_acceptance_report_body(&fence.body),
                Ok((Some(_), _))
            ) {
                return output.get(..fence.index).unwrap_or("").trim_end().to_string();
            }
        }
        // Fallbacks (acceptance.ts:511-514): a trailing acceptance-report fence, then a trailing
        // ACCEPTANCE_REPORT: {...} marker, then trimEnd.
        let stripped = strip_trailing_acceptance_report_fence(output);
        let stripped = strip_trailing_acceptance_marker(&stripped);
        stripped.trim_end().to_string()
    }

    /// `/\n?```acceptance[-_]report\s*\n[\s\S]*?```\s*$/i` (acceptance.ts:792).
    fn strip_trailing_acceptance_report_fence(output: &str) -> String {
        let matches = fenced_matches(output, ACCEPTANCE_REPORT_FENCE_TAGS, true, true);
        if let Some(fence) = matches
            .into_iter()
            .rev()
            .find(|m| output.get(m.end..).is_none_or(|tail| tail.is_empty()))
        {
            return output.get(..fence.index).unwrap_or("").to_string();
        }
        output.to_string()
    }

    /// `/\n?ACCEPTANCE_REPORT\s*:\s*\{[\s\S]*\}\s*$/i` (acceptance.ts:513) — greedy `{...}` to the
    /// LAST `}` before trailing whitespace/end.
    fn strip_trailing_acceptance_marker(output: &str) -> String {
        let Some(marker_index) = find_acceptance_report_marker(output) else {
            return output.to_string();
        };
        let Some(brace_rel) = output.get(marker_index..).and_then(|s| s.find('{')) else {
            return output.to_string();
        };
        // Between the marker and the `{`, only `\s*:\s*` is allowed.
        let between = output.get(marker_index + "ACCEPTANCE_REPORT".len()..marker_index + brace_rel).unwrap_or("");
        let between_ok = {
            let t = between.trim();
            t == ":"
        };
        if !between_ok {
            return output.to_string();
        }
        // Greedy: last `}` such that everything after it is whitespace.
        let Some(last_close) = output.rfind('}') else {
            return output.to_string();
        };
        if output.get(last_close + 1..).is_none_or(|tail| tail.trim().is_empty()) && last_close > marker_index {
            let start = if marker_index > 0 && output.as_bytes().get(marker_index - 1) == Some(&b'\n') {
                marker_index - 1
            } else {
                marker_index
            };
            return output.get(..start).unwrap_or("").to_string();
        }
        output.to_string()
    }

    /// Strip acceptance-report fences from every text part of an assistant message value in place
    /// (`stripAcceptanceReportsFromMessages`, execution.ts:822/1097) — used by the delivered-output
    /// path so a stored transcript never shows the machine report JSON either.
    #[must_use]
    pub fn strip_acceptance_report_from_message_text(text: &str) -> String {
        strip_acceptance_report(text)
    }

    // --------------------------------------------------------------------------------------------
    // Report-driven runtime checks (acceptance.ts:911-966)
    // --------------------------------------------------------------------------------------------

    /// `reportEvidencePresent` (acceptance.ts:632-644).
    fn report_evidence_present(report: &AcceptanceReport, kind: AcceptanceEvidenceKind) -> bool {
        match kind {
            AcceptanceEvidenceKind::ChangedFiles => {
                report.changed_files.as_ref().is_some_and(|v| !v.is_empty())
            }
            AcceptanceEvidenceKind::TestsAdded => report
                .tests_added_or_updated
                .as_ref()
                .is_some_and(|v| !v.is_empty()),
            AcceptanceEvidenceKind::CommandsRun => {
                report.commands_run.as_ref().is_some_and(|v| !v.is_empty())
            }
            AcceptanceEvidenceKind::ValidationOutput => report
                .validation_output
                .as_ref()
                .is_some_and(|v| !v.is_empty()),
            AcceptanceEvidenceKind::ResidualRisks => report.residual_risks.is_some(),
            AcceptanceEvidenceKind::NoStagedFiles => report.no_staged_files == Some(true),
            AcceptanceEvidenceKind::DiffSummary => report
                .diff_summary
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            AcceptanceEvidenceKind::ReviewFindings => report.review_findings.is_some(),
            AcceptanceEvidenceKind::ManualNotes => report
                .manual_notes
                .as_deref()
                .or(report.notes.as_deref())
                .is_some_and(|s| !s.trim().is_empty()),
        }
    }

    /// `checkCriteriaSatisfied` (acceptance.ts:911-919).
    ///
    /// Public because the live gate ([`super::evaluate_acceptance`]'s `Checked` rung) runs this
    /// same check over [`super::AcceptanceContract::criteria`] — there is exactly one criteria
    /// rule and both evaluators must share it.
    #[must_use]
    pub fn check_criteria_satisfied(
        criteria: &[ResolvedAcceptanceGate],
        report: &AcceptanceReport,
    ) -> Vec<AcceptanceRuntimeCheck> {
        // `acceptance.ts:912-914` @v0.43.0: BOTH sides go through `normalizedToken`, so a declared
        // `c 1` matches a reported `C_1`. The report side is already normalized by
        // `normalize_criterion_report`; the DECLARED side is not, so it is normalized here.
        let reported: std::collections::HashMap<String, &CriterionReport> = report
            .criteria_satisfied
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|item| {
                item.id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .map(|id| (normalized_token(id), item))
            })
            .collect();
        criteria
            .iter()
            .filter(|criterion| criterion.severity != GateSeverity::Recommended)
            .map(|criterion| {
                let id = format!("criterion:{}", criterion.id);
                match reported.get(&normalized_token(&criterion.id)) {
                    Option::None => AcceptanceRuntimeCheck {
                        id,
                        status: RuntimeCheckStatus::Failed,
                        message: format!(
                            "Required criterion '{}' was not reported.",
                            criterion.id
                        ),
                    },
                    Some(item) if item.status != CriterionStatus::Satisfied => AcceptanceRuntimeCheck {
                        id,
                        status: RuntimeCheckStatus::Failed,
                        message: format!(
                            "Required criterion '{}' was reported as {}.",
                            criterion.id,
                            criterion_status_str(item.status)
                        ),
                    },
                    Some(_) => AcceptanceRuntimeCheck {
                        id,
                        status: RuntimeCheckStatus::Passed,
                        message: format!("Required criterion '{}' satisfied.", criterion.id),
                    },
                }
            })
            .collect()
    }

    fn criterion_status_str(status: CriterionStatus) -> &'static str {
        match status {
            CriterionStatus::Satisfied => "satisfied",
            CriterionStatus::NotSatisfied => "not-satisfied",
            CriterionStatus::NotApplicable => "not-applicable",
        }
    }

    /// `checkNoStagedFiles` (acceptance.ts:939-948) — REAL `git status --short` subprocess in `cwd`.
    async fn check_no_staged_files(cwd: &Path) -> AcceptanceRuntimeCheck {
        let output = tokio::process::Command::new("git")
            .arg("status")
            .arg("--short")
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await;
        let Ok(output) = output else {
            return AcceptanceRuntimeCheck {
                id: "no-staged-files".to_string(),
                status: RuntimeCheckStatus::NotApplicable,
                message: "git status unavailable; no staged-files check skipped".to_string(),
            };
        };
        if !output.status.success() {
            return AcceptanceRuntimeCheck {
                id: "no-staged-files".to_string(),
                status: RuntimeCheckStatus::NotApplicable,
                message: "git status unavailable; no staged-files check skipped".to_string(),
            };
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        // pi: lines with length>=2 and line[0] !== " " and line[0] !== "?" are staged.
        let staged: Vec<&str> = stdout
            .split(['\n', '\r'])
            .filter(|line| {
                let mut chars = line.chars();
                let first = chars.next();
                line.chars().count() >= 2
                    && !matches!(first, Some(' ') | Some('?'))
            })
            .collect();
        if staged.is_empty() {
            AcceptanceRuntimeCheck {
                id: "no-staged-files".to_string(),
                status: RuntimeCheckStatus::Passed,
                message: "No staged files detected.".to_string(),
            }
        } else {
            AcceptanceRuntimeCheck {
                id: "no-staged-files".to_string(),
                status: RuntimeCheckStatus::Failed,
                message: format!("Staged files present: {}", staged.join(", ")),
            }
        }
    }

    /// `runStructuralChecks` (acceptance.ts:950-966).
    ///
    /// Takes the evidence list rather than the whole [`ResolvedAcceptanceConfig`] — upstream reads
    /// nothing else off `acceptance` (`for (const kind of acceptance.evidence)` plus the
    /// `acceptance.evidence.includes("no-staged-files")` branch) — so the live gate, whose contract
    /// is an [`super::AcceptanceContract`] rather than a `ResolvedAcceptanceConfig`, can call this
    /// exact function instead of growing a second copy of the evidence rules.
    #[must_use]
    pub async fn run_structural_checks(
        evidence: &[AcceptanceEvidenceKind],
        report: &AcceptanceReport,
        cwd: &Path,
    ) -> Vec<AcceptanceRuntimeCheck> {
        let mut checks: Vec<AcceptanceRuntimeCheck> = Vec::new();
        for kind in evidence {
            let present = report_evidence_present(report, *kind);
            checks.push(AcceptanceRuntimeCheck {
                id: format!("evidence:{}", kind.as_str()),
                status: if present {
                    RuntimeCheckStatus::Passed
                } else {
                    RuntimeCheckStatus::Failed
                },
                message: if present {
                    format!("{} evidence present.", kind.as_str())
                } else {
                    format!("{} evidence missing from child report.", kind.as_str())
                },
            });
        }
        if evidence.contains(&AcceptanceEvidenceKind::NoStagedFiles) {
            checks.push(check_no_staged_files(cwd).await);
        }
        checks
    }

    // --------------------------------------------------------------------------------------------
    // aggregateAcceptanceReport (acceptance.ts:671-711)
    // --------------------------------------------------------------------------------------------

    /// One child-run summary consumed by [`aggregate_acceptance_report`] (acceptance.ts:681-683 —
    /// `Pick<SingleResult, "agent" | "acceptance" | "error" | "exitCode">`).
    #[derive(Debug, Clone)]
    pub struct AggregateChild {
        pub agent: String,
        pub acceptance: Option<AcceptanceLedger>,
        pub error: Option<String>,
        pub exit_code: i32,
    }

    fn trim_output(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Option::None;
        }
        if trimmed.chars().count() > 12_000 {
            let head: String = trimmed.chars().take(12_000).collect();
            Some(format!("{head}\n...[truncated]"))
        } else {
            Some(trimmed.to_string())
        }
    }

    fn unique_strings(items: Vec<Option<String>>) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for text in items.into_iter().flatten() {
            let trimmed = text.trim();
            if !trimmed.is_empty() && !seen.iter().any(|s| s == trimmed) {
                seen.push(trimmed.to_string());
            }
        }
        seen
    }

    /// `aggregateAcceptanceReport` (acceptance.ts:1000-1030).
    #[must_use]
    pub fn aggregate_acceptance_report(
        results: &[AggregateChild],
        notes: Option<&str>,
    ) -> AcceptanceReport {
        let child_reports: Vec<&AcceptanceReport> = results
            .iter()
            .filter_map(|r| r.acceptance.as_ref().and_then(|l| l.child_report.as_ref()))
            .collect();
        let blockers: Vec<&AggregateChild> = results
            .iter()
            .filter(|r| {
                r.exit_code != 0
                    || r.acceptance
                        .as_ref()
                        .is_some_and(|l| l.status == AcceptanceLedgerStatus::Rejected)
            })
            .collect();
        let successful = !results.is_empty() && blockers.is_empty();

        let mut criteria: Vec<CriterionReport> = vec![
            CriterionReport {
                id: Some("criterion-1".to_string()),
                status: if successful {
                    CriterionStatus::Satisfied
                } else {
                    CriterionStatus::NotSatisfied
                },
                evidence: if successful {
                    format!(
                        "All {} dynamic child run(s) completed without child or acceptance blockers.",
                        results.len()
                    )
                } else {
                    "Dynamic fanout produced no accepted child evidence.".to_string()
                },
            },
            CriterionReport {
                id: Some("criterion-2".to_string()),
                status: if successful {
                    CriterionStatus::Satisfied
                } else {
                    CriterionStatus::NotSatisfied
                },
                evidence: if successful {
                    "Collected child acceptance evidence for aggregate review.".to_string()
                } else {
                    "Dynamic fanout produced no aggregate review evidence.".to_string()
                },
            },
        ];
        for (index, result) in results.iter().enumerate() {
            let child_ok = result.exit_code == 0
                && result
                    .acceptance
                    .as_ref()
                    .is_none_or(|l| l.status != AcceptanceLedgerStatus::Rejected);
            let status_str = result
                .acceptance
                .as_ref()
                .map(|l| ledger_status_str(l.status))
                .unwrap_or("unreported");
            let err_suffix = result
                .error
                .as_deref()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default();
            criteria.push(CriterionReport {
                id: Some(format!("child-{}", index + 1)),
                status: if child_ok {
                    CriterionStatus::Satisfied
                } else {
                    CriterionStatus::NotSatisfied
                },
                evidence: format!("{}: acceptance {status_str}{err_suffix}", result.agent),
            });
        }

        let changed_files = unique_strings(
            child_reports
                .iter()
                .flat_map(|r| r.changed_files.clone().unwrap_or_default())
                .map(Some)
                .collect(),
        );
        let tests = unique_strings(
            child_reports
                .iter()
                .flat_map(|r| r.tests_added_or_updated.clone().unwrap_or_default())
                .map(Some)
                .collect(),
        );
        let commands: Vec<CommandRunReport> = child_reports
            .iter()
            .flat_map(|r| r.commands_run.clone().unwrap_or_default())
            .collect();
        let validation = unique_strings(
            child_reports
                .iter()
                .flat_map(|r| r.validation_output.clone().unwrap_or_default())
                .map(Some)
                .collect(),
        );
        let mut residual_items: Vec<Option<String>> = child_reports
            .iter()
            .flat_map(|r| r.residual_risks.clone().unwrap_or_default())
            .map(Some)
            .collect();
        for blocker in &blockers {
            residual_items.push(Some(format!(
                "{}: {}",
                blocker.agent,
                blocker
                    .error
                    .clone()
                    .unwrap_or_else(|| "child or acceptance gate failed".to_string())
            )));
        }
        let residual = unique_strings(residual_items);
        let review_findings = unique_strings(
            child_reports
                .iter()
                .flat_map(|r| r.review_findings.clone().unwrap_or_default())
                .map(Some)
                .collect(),
        );

        AcceptanceReport {
            criteria_satisfied: Some(criteria),
            changed_files: Some(changed_files),
            tests_added_or_updated: Some(tests),
            commands_run: Some(commands),
            validation_output: Some(validation),
            residual_risks: Some(residual),
            no_staged_files: Some(
                !child_reports.is_empty()
                    && child_reports.iter().all(|r| r.no_staged_files == Some(true)),
            ),
            diff_summary: Option::None,
            review_findings: Some(review_findings),
            manual_notes: Some(notes.map(str::to_string).unwrap_or_else(|| {
                format!(
                    "Aggregated acceptance evidence from {} dynamic fanout child run(s).",
                    results.len()
                )
            })),
            notes: notes.map(str::to_string),
        }
    }

    fn ledger_status_str(status: AcceptanceLedgerStatus) -> &'static str {
        match status {
            AcceptanceLedgerStatus::Pending => "pending",
            AcceptanceLedgerStatus::NotRequired => "not-required",
            AcceptanceLedgerStatus::Claimed => "claimed",
            AcceptanceLedgerStatus::Attested => "attested",
            AcceptanceLedgerStatus::Checked => "checked",
            AcceptanceLedgerStatus::Verified => "verified",
            AcceptanceLedgerStatus::ReviewRequired => "review-required",
            AcceptanceLedgerStatus::Reviewed => "reviewed",
            AcceptanceLedgerStatus::Accepted => "accepted",
            AcceptanceLedgerStatus::Rejected => "rejected",
        }
    }

    // --------------------------------------------------------------------------------------------
    // G80: verify-command secret redaction (acceptance.ts:974-994)
    // --------------------------------------------------------------------------------------------

    /// The alternation inside upstream's `SENSITIVE_ENV_KEY_PATTERN`
    /// (`acceptance.ts:974` @v0.43.0):
    ///
    /// ```text
    /// /(?:^|_)(?:TOKEN|SECRET|PASSWORD|PASS|AUTH|CREDENTIAL|COOKIE|SESSION|PRIVATE|API_KEY|ACCESS_KEY)(?:_|$)/i
    /// ```
    ///
    /// Copied VERBATIM and in upstream's order. This list is a security boundary — a verify
    /// command's captured stdout/stderr goes straight into the acceptance ledger and from there
    /// into a transcript, so anything this list misses is a credential that leaks. Do not "improve"
    /// it locally; change it only to track upstream.
    const SENSITIVE_ENV_KEY_WORDS: [&str; 11] = [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASS",
        "AUTH",
        "CREDENTIAL",
        "COOKIE",
        "SESSION",
        "PRIVATE",
        "API_KEY",
        "ACCESS_KEY",
    ];

    /// `SENSITIVE_ENV_KEY_PATTERN.test(key)` (`acceptance.ts:974,985`), re-expressed as a scan so
    /// the crate needs no regex dependency.
    ///
    /// The pattern is unanchored and case-insensitive, so it matches when ANY word in
    /// [`SENSITIVE_ENV_KEY_WORDS`] occurs at a `_`-or-boundary-delimited position anywhere in the
    /// key: `GITHUB_TOKEN` and `TOKEN_FILE` and `AWS_SECRET_ACCESS_KEY` all match, while
    /// `TOKENIZER` and `PASSAGE` do not (`I`/`A` is neither `_` nor end-of-string).
    ///
    /// `to_ascii_uppercase` is what makes the `i` flag faithful without changing byte offsets —
    /// env key names are ASCII, and a non-ASCII byte is left alone and simply never matches.
    #[must_use]
    fn is_sensitive_env_key(key: &str) -> bool {
        let upper = key.to_ascii_uppercase();
        let bytes = upper.as_bytes();
        for word in SENSITIVE_ENV_KEY_WORDS {
            let needle = word.as_bytes();
            if needle.len() > bytes.len() {
                continue;
            }
            for start in 0..=(bytes.len() - needle.len()) {
                let end = start + needle.len();
                if bytes.get(start..end) != Some(needle) {
                    continue;
                }
                // `(?:^|_)` before, `(?:_|$)` after.
                let left = start == 0 || bytes.get(start - 1) == Some(&b'_');
                let right = end == bytes.len() || bytes.get(end) == Some(&b'_');
                if left && right {
                    return true;
                }
            }
        }
        false
    }

    /// `effectiveVerifyEnv` (`acceptance.ts:976-981`): `{ ...process.env, ...(env ?? {}) }` — the
    /// command's declared pairs layered OVER the inherited environment, never replacing it.
    ///
    /// Upstream's `flatMap` drops any `process.env` entry whose value is not a string; the Rust
    /// analog is dropping any `vars_os` pair that is not valid UTF-8 (which is also why this reads
    /// `vars_os` rather than `vars`, whose iterator panics on exactly that input — the no-panic
    /// policy forbids it).
    #[must_use]
    fn effective_verify_env(
        env: Option<&std::collections::BTreeMap<String, String>>,
    ) -> std::collections::BTreeMap<String, String> {
        let mut merged: std::collections::BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(key, value)| {
                Some((key.into_string().ok()?, value.into_string().ok()?))
            })
            .collect();
        if let Some(declared) = env {
            for (key, value) in declared {
                merged.insert(key.clone(), value.clone());
            }
        }
        merged
    }

    /// `verifyRedactionEnv` (`acceptance.ts:983-987`): the effective environment filtered down to
    /// the entries whose KEY looks sensitive and whose VALUE is at least 4 long — the length floor
    /// upstream applies so that a short/degenerate value (`"1"`, `"on"`) cannot blanket-redact
    /// every occurrence of that substring in otherwise-innocent output.
    ///
    /// JS `.length` counts UTF-16 units where Rust `.len()` counts bytes; the two agree exactly for
    /// the ASCII every real credential is made of, and both are monotone in string size, so the
    /// longest-first ordering below is preserved either way.
    #[must_use]
    fn verify_redaction_env(
        env: Option<&std::collections::BTreeMap<String, String>>,
    ) -> Vec<String> {
        effective_verify_env(env)
            .into_iter()
            .filter(|(key, value)| value.len() >= 4 && is_sensitive_env_key(key))
            .map(|(_, value)| value)
            .collect()
    }

    /// `redactVerifyEnv` (`acceptance.ts:989-994`): replace every occurrence of every sensitive
    /// environment VALUE in `value` with `[REDACTED]`.
    ///
    /// The de-duplicated secret list is sorted LONGEST FIRST (upstream
    /// `.sort((left, right) => right.length - left.length)`), which is load-bearing: when one
    /// secret is a prefix of another, redacting the short one first would leave the remainder of
    /// the long one in the output. `str::replace` is a literal replacement, exactly like
    /// `String.prototype.replaceAll` with a string (not regex) pattern.
    #[must_use]
    pub fn redact_verify_env(
        value: &str,
        env: Option<&std::collections::BTreeMap<String, String>>,
    ) -> String {
        let mut secrets = verify_redaction_env(env);
        // `[...new Set(...)]` — dedupe. Sorting first makes `dedup` total, and the subsequent
        // stable length sort then leaves equal-length secrets in a deterministic order.
        secrets.sort();
        secrets.dedup();
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        let mut redacted = value.to_string();
        for secret in secrets {
            // `.filter(Boolean)` (`acceptance.ts:991`) — an empty secret would otherwise splice
            // `[REDACTED]` between every character.
            if secret.is_empty() {
                continue;
            }
            redacted = redacted.replace(&secret, "[REDACTED]");
        }
        redacted
    }

    // --------------------------------------------------------------------------------------------
    // G80: per-workspace memoization of verify results (acceptance.ts:1032-1132)
    // --------------------------------------------------------------------------------------------

    /// `hash` (`acceptance.ts:1034-1036`): lowercase hex sha256.
    #[must_use]
    fn hash_bytes(value: &[u8]) -> String {
        use sha2::{Digest as _, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(value);
        let digest = hasher.finalize();
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The two values upstream requires BOTH of before it will memoize anything at all
    /// (`if (!workspaceState || !options.artifactsDir || !options.runId)`, `acceptance.ts:1085`):
    /// the run's artifacts root and the run id that scopes the cache within it.
    ///
    /// Passing `None` for the whole context is upstream's "no artifacts configured" case — every
    /// verify command then executes for real, exactly as it did before memoization existed. That
    /// is also what pi's chain-execution group gate does: its two `evaluateAcceptance` calls
    /// (`chain-execution.ts:1037-1046,1233-1242`) pass neither field.
    #[derive(Debug, Clone, Copy)]
    pub struct VerifyMemoContext<'a> {
        /// pi `options.artifactsDir` — the run's artifacts root. Memo artifacts land under
        /// `<artifacts_dir>/acceptance/verify/<run_id>/<cacheKey>.json` (`acceptance.ts:1102`).
        pub artifacts_dir: &'a Path,
        /// pi `options.runId`.
        pub run_id: &'a str,
    }

    /// `readVerifyWorkspaceState` (`acceptance.ts:1046-1060`): identify the git working tree
    /// `cwd` sits in, as `HEAD` plus a hash of the full uncommitted diff.
    ///
    /// Returns `None` — which disables memoization for this command entirely — when `cwd` is not
    /// inside a git checkout, when either `git` invocation fails, or when `HEAD` is empty (an
    /// unborn branch). A non-git workspace has no cheap identity to key a cache on, so upstream
    /// declines to guess one.
    ///
    /// **[CYRUP-DELTA: mechanism]** upstream uses `spawnSync` and hashes the diff after decoding it
    /// as UTF-8; this awaits `tokio::process::Command` (blocking the async executor on three git
    /// invocations is not an option here) and hashes the diff's RAW BYTES, which is strictly more
    /// faithful for the `--binary` diffs the flag exists to produce — a lossy decode would collapse
    /// distinct binary blobs onto the same replacement characters and therefore the same key.
    pub async fn read_verify_workspace_state(cwd: &Path) -> Option<VerifyWorkspaceState> {
        let repo = tokio::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(cwd)
            .output()
            .await
            .ok()?;
        if !repo.status.success() {
            return Option::None;
        }
        let repo_root_raw = String::from_utf8(repo.stdout).ok()?;
        let repo_root_raw = repo_root_raw.trim();
        if repo_root_raw.is_empty() {
            return Option::None;
        }
        // `fs.realpathSync` (`acceptance.ts:1049`) — both sides are canonicalized so the
        // `path.relative` below cannot be defeated by a symlinked cwd.
        let repo_root = std::fs::canonicalize(repo_root_raw).ok()?;

        let head = tokio::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_root)
            .output()
            .await
            .ok()?;
        let diff = tokio::process::Command::new("git")
            .args(["diff", "--binary", "--full-index", "HEAD", "--"])
            .current_dir(&repo_root)
            .output()
            .await
            .ok()?;
        if !head.status.success() || !diff.status.success() {
            return Option::None;
        }
        let head_text = String::from_utf8(head.stdout).ok()?;
        let head_text = head_text.trim();
        if head_text.is_empty() {
            return Option::None;
        }

        // `path.relative(repoRoot, fs.realpathSync(cwd)) || "."` (`acceptance.ts:1056`). `cwd` is
        // always inside `repoRoot` here — `repoRoot` was derived by running `rev-parse` FROM it —
        // so a plain prefix strip is exactly `path.relative`, and `""` becomes `"."`.
        let cwd_real = std::fs::canonicalize(cwd).ok()?;
        let relative = cwd_real.strip_prefix(&repo_root).ok()?;
        let cwd_relative = if relative.as_os_str().is_empty() {
            ".".to_string()
        } else {
            relative.to_string_lossy().into_owned()
        };

        Some(VerifyWorkspaceState {
            kind: VerifyWorkspaceKind::GitTracked,
            repo_root: repo_root.to_string_lossy().into_owned(),
            cwd_relative,
            head: head_text.to_string(),
            diff_hash: hash_bytes(&diff.stdout),
        })
    }

    /// The shape marker stamped into a memo artifact's `resultShape` field.
    ///
    /// **[CYRUP-DELTA: mechanism]** upstream has exactly ONE verify-result type, so its artifact
    /// needs no discriminant, and as of the verify-layer collapse so does this crate — every writer
    /// and every reader is [`AcceptanceVerifyResult`]. The marker survives because artifacts
    /// written by the previous build DO carry a second value (`"verify-command-result"`, the
    /// retired lattice shape), and those must read as a clean MISS — re-run the command — rather
    /// than be coerced through serde into a shape they were never written as. It is an opaque field
    /// of a private artifact; nothing observable depends on it.
    const MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT: &str = "acceptance-verify-result";

    /// The everything-but-the-result half of a memo artifact, shared by both result shapes.
    ///
    /// Mirrors upstream's written object (`acceptance.ts:1115-1126`) field for field, minus
    /// `result` (supplied by the caller) plus `resultShape` (see
    /// [`MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT`]).
    pub(crate) struct MemoIdentity {
        pub(crate) cache_key: String,
        pub(crate) artifact_path: PathBuf,
        pub(crate) env_keys: Vec<String>,
        pub(crate) env_hash: String,
        pub(crate) timeout_ms: u64,
        pub(crate) allow_failure: bool,
        pub(crate) workspace_state: VerifyWorkspaceState,
    }

    impl MemoIdentity {
        /// `acceptance.ts:1088-1102`: derive this command's cache key and artifact path against an
        /// already-read [`VerifyWorkspaceState`].
        ///
        /// The key covers everything that can change the command's OUTCOME: its text, the
        /// repo-relative directory it runs in, the names of the env keys it declares, a hash of the
        /// entire effective environment, its timeout, its `allowFailure` flag, `HEAD`, and the
        /// working-tree diff hash. Note `env_keys` records only NAMES (the ledger is
        /// transcript-visible) while `env_hash` covers every VALUE — that split is upstream's, and
        /// it is what lets a rotated credential invalidate the memo without ever being written
        /// down.
        ///
        /// **[CYRUP-DELTA: mechanism]** the key is a sha256 over `serde_json`'s rendering of the
        /// same field set rather than over V8's `JSON.stringify` of it, so the digest VALUE differs
        /// from pi's. Nothing compares the two: a cache key is only ever matched against another
        /// key produced by the same build, and upstream re-checks `cached.cacheKey === cacheKey`
        /// on read for exactly that reason.
        pub(crate) fn derive(
            command: &AcceptanceVerifyCommand,
            memo: VerifyMemoContext<'_>,
            workspace_state: VerifyWorkspaceState,
            result_shape: &str,
        ) -> Self {
            // `Object.keys(command.env ?? {}).sort()` (`acceptance.ts:1088`) — a `BTreeMap` is
            // already sorted.
            let env_keys: Vec<String> = command
                .env
                .as_ref()
                .map(|env| env.keys().cloned().collect())
                .unwrap_or_default();
            // `hash(JSON.stringify(<effective env, key-sorted>))` (`acceptance.ts:1089`).
            let effective = effective_verify_env(command.env.as_ref());
            let env_hash = hash_bytes(
                serde_json::to_string(&effective)
                    .unwrap_or_default()
                    .as_bytes(),
            );
            let timeout_ms = command.timeout_ms.unwrap_or(DEFAULT_VERIFY_TIMEOUT_MS);
            let allow_failure = command.allow_failure == Some(true);
            let key_material = serde_json::json!({
                "version": 1,
                "command": command.command,
                "cwdRelative": workspace_state.cwd_relative,
                "envKeys": env_keys,
                "envHash": env_hash,
                "timeoutMs": timeout_ms,
                "allowFailure": allow_failure,
                "head": workspace_state.head,
                "diffHash": workspace_state.diff_hash,
                "resultShape": result_shape,
            });
            let cache_key = hash_bytes(
                serde_json::to_string(&key_material)
                    .unwrap_or_default()
                    .as_bytes(),
            );
            // `path.join(artifactsDir, "acceptance", "verify", runId, `${cacheKey}.json`)`
            // (`acceptance.ts:1102`).
            let artifact_path = memo
                .artifacts_dir
                .join("acceptance")
                .join("verify")
                .join(memo.run_id)
                .join(format!("{cache_key}.json"));
            Self {
                cache_key,
                artifact_path,
                env_keys,
                env_hash,
                timeout_ms,
                allow_failure,
                workspace_state,
            }
        }

        /// The `result` payload of a matching memo artifact, or `None` for any miss.
        ///
        /// Upstream's read is wrapped in a bare `try {} catch {}` whose comment says it out loud —
        /// *"A cache miss or unreadable artifact must not prevent host verification"*
        /// (`acceptance.ts:1108-1110`) — so an absent file, malformed JSON, a stale `cacheKey` or a
        /// foreign `resultShape` are all just misses.
        pub(crate) fn read_cached(&self, result_shape: &str) -> Option<serde_json::Value> {
            let raw = std::fs::read(&self.artifact_path).ok()?;
            let cached: serde_json::Value = serde_json::from_slice(&raw).ok()?;
            if cached.get("cacheKey").and_then(serde_json::Value::as_str)
                != Some(self.cache_key.as_str())
            {
                return Option::None;
            }
            // An artifact with no marker predates the field; treat it as the pi-shaped default so
            // this stays a pure addition.
            let shape = cached
                .get("resultShape")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT);
            if shape != result_shape {
                return Option::None;
            }
            cached.get("result").cloned()
        }

        /// Write the memo artifact (`acceptance.ts:1113-1126`), returning the error TEXT upstream
        /// puts on `artifactError` when the write fails.
        ///
        /// Best-effort by construction: the command has already run and its real exit code is
        /// already known, so a failure here can only cost a future re-run, never a wrong verdict.
        pub(crate) fn write_cached(
            &self,
            command: &AcceptanceVerifyCommand,
            result_shape: &str,
            result: &serde_json::Value,
        ) -> Result<(), String> {
            let payload = serde_json::json!({
                "version": 1,
                "cacheKey": self.cache_key,
                "command": command.command,
                "cwdRelative": self.workspace_state.cwd_relative,
                "envKeys": self.env_keys,
                "envHash": self.env_hash,
                "timeoutMs": self.timeout_ms,
                "allowFailure": self.allow_failure,
                "workspaceState": self.workspace_state,
                "resultShape": result_shape,
                "result": result,
            });
            let text = serde_json::to_string_pretty(&payload).map_err(|err| err.to_string())?;
            if let Some(parent) = self.artifact_path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::write(&self.artifact_path, text).map_err(|err| err.to_string())
        }
    }

    /// `runMemoizedVerifyCommand` (`acceptance.ts:1072-1132`): replay a verify command's recorded
    /// result when the workspace has not changed since it was recorded, otherwise run it for real
    /// and record the outcome.
    ///
    /// Falls straight through to [`run_verify_command`] — no artifact read, no artifact write, no
    /// evidence fields — whenever there is no memo context or the cwd is not a git working tree
    /// (`acceptance.ts:1085-1087`). The memoized replay carries the recorded `exitCode`, `status`,
    /// `stdout`, `stderr` and `durationMs` but re-stamps `id`/`command`/`cwd` from the CURRENT
    /// command (`acceptance.ts:1106`), so a renamed criterion id still reports under its new name.
    pub async fn run_memoized_verify_command(
        command: &AcceptanceVerifyCommand,
        default_cwd: &Path,
        memo: Option<VerifyMemoContext<'_>>,
    ) -> AcceptanceVerifyResult {
        let cwd = resolve_verify_cwd(command, default_cwd);
        let Some(memo) = memo else {
            return run_verify_command(command, default_cwd).await;
        };
        // `try { workspaceState = readVerifyWorkspaceState(cwd) } catch { undefined }`
        // (`acceptance.ts:1079-1084`).
        let Some(workspace_state) = read_verify_workspace_state(&cwd).await else {
            return run_verify_command(command, default_cwd).await;
        };
        let identity = MemoIdentity::derive(
            command,
            memo,
            workspace_state,
            MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT,
        );

        if let Some(cached) = identity.read_cached(MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT)
            // `isCachedVerifyResult` (`acceptance.ts:1062-1070`) asserts id/command are strings,
            // `exitCode` is a number or an explicit null, `status` is one of the four literals and
            // `durationMs` is a number. Every one of those is a REQUIRED field of
            // `AcceptanceVerifyResult` (only `cwd` and the evidence fields carry `#[serde(default)]`),
            // so a successful deserialization IS that predicate.
            && let Ok(result) = serde_json::from_value::<AcceptanceVerifyResult>(cached)
        {
            return AcceptanceVerifyResult {
                id: command.id.clone(),
                command: command.command.clone(),
                cwd: Some(cwd.display().to_string()),
                artifact_path: Some(identity.artifact_path.display().to_string()),
                cache_key: Some(identity.cache_key.clone()),
                memoized: Some(true),
                env_keys: Some(identity.env_keys.clone()),
                env_hash: Some(identity.env_hash.clone()),
                workspace_state: Some(identity.workspace_state.clone()),
                artifact_error: Option::None,
                ..result
            };
        }

        let result = run_verify_command(command, default_cwd).await;
        let mut evidenced = AcceptanceVerifyResult {
            artifact_path: Some(identity.artifact_path.display().to_string()),
            cache_key: Some(identity.cache_key.clone()),
            memoized: Some(false),
            env_keys: Some(identity.env_keys.clone()),
            env_hash: Some(identity.env_hash.clone()),
            workspace_state: Some(identity.workspace_state.clone()),
            artifact_error: Option::None,
            ..result
        };
        let payload = serde_json::to_value(&evidenced).unwrap_or(serde_json::Value::Null);
        if let Err(message) =
            identity.write_cached(command, MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT, &payload)
        {
            // `evidenced.artifactError = …; delete evidenced.artifactPath;`
            // (`acceptance.ts:1128-1129`) — never claim an artifact that is not there.
            evidenced.artifact_error = Some(message);
            evidenced.artifact_path = Option::None;
        }
        evidenced
    }

    // --------------------------------------------------------------------------------------------
    // runVerifyCommand (acceptance.ts:1134-1208) — REAL subprocess execution
    // --------------------------------------------------------------------------------------------

    /// `DEFAULT_VERIFY_TIMEOUT_MS` (`acceptance.ts:1032`): the bound applied to a `verify[]`
    /// command that declares no `timeoutMs` of its own (`command.timeoutMs ?? 120_000`,
    /// `acceptance.ts:1090,1179`). A declared `timeoutMs` wins; this only keeps a hung *undeclared*
    /// verification command from blocking the acceptance gate — and therefore the whole run —
    /// indefinitely.
    pub const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 120_000;

    /// `"Acceptance verification timed out."` (`acceptance.ts:1174,1203`) — upstream's last-resort
    /// `stderr` for a killed verify command, used when the command printed nothing to stderr of its
    /// own and no `abortMessage` was supplied.
    pub const VERIFY_TIMED_OUT_MESSAGE: &str = "Acceptance verification timed out.";

    /// The `stderr` for the one timeout shape upstream has no counterpart for: the command EXITED,
    /// but something it backgrounded still holds its stdout/stderr, so the capture could not reach
    /// EOF before the deadline. Upstream reaches the same `timed-out` verdict here (its `"close"`
    /// event likewise never fires) but cannot distinguish the case; naming it keeps a genuinely
    /// confusing failure diagnosable.
    pub const VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE: &str =
        "Acceptance verification timed out: the command exited, but a process it backgrounded \
         still holds its stdout/stderr.";

    /// `command.cwd ? path.resolve(defaultCwd, command.cwd) : defaultCwd`
    /// (`acceptance.ts:1078,1137`) — `path.resolve` returns an absolute segment verbatim and joins
    /// a relative one onto the base, which is what `Path::join` does.
    #[must_use]
    fn resolve_verify_cwd(command: &AcceptanceVerifyCommand, default_cwd: &Path) -> PathBuf {
        match command.cwd.as_deref() {
            Some(rel) => {
                let path = Path::new(rel);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    default_cwd.join(path)
                }
            }
            Option::None => default_cwd.to_path_buf(),
        }
    }

    impl AcceptanceVerifyResult {
        /// Whether this result REJECTS the run — upstream's
        /// `verifyRuns.some((run) => run.status === "failed" || run.status === "timed-out")`
        /// (`acceptance.ts:1297`, and identically `:1361`). A passed command and an
        /// `allowed-failure` command both return `false`: `allowFailure` is exactly the authored
        /// opt-out from rejection, and upstream's status ternary already folded it in
        /// (`acceptance.ts:1193`).
        #[must_use]
        pub fn rejects(&self) -> bool {
            matches!(
                self.status,
                VerifyRunStatus::Failed | VerifyRunStatus::TimedOut
            )
        }

        /// Everything upstream's `finish(...)` (`acceptance.ts:1150-1163`) resolves, with NO
        /// memoization evidence attached — the plain shape a command executed outside a memo
        /// context reports. [`run_memoized_verify_command`] stamps the evidence on afterwards.
        fn unmemoized(
            command: &AcceptanceVerifyCommand,
            cwd: Option<String>,
            exit_code: Option<i32>,
            status: VerifyRunStatus,
            stdout: Option<String>,
            stderr: Option<String>,
            duration_ms: u128,
        ) -> Self {
            Self {
                id: command.id.clone(),
                command: command.command.clone(),
                cwd,
                exit_code,
                status,
                stdout,
                stderr,
                duration_ms,
                artifact_path: Option::None,
                cache_key: Option::None,
                memoized: Option::None,
                env_keys: Option::None,
                env_hash: Option::None,
                workspace_state: Option::None,
                artifact_error: Option::None,
            }
        }
    }

    /// `runVerifyCommand` (`acceptance.ts:1134-1208`): execute one `verify[]` command as a REAL
    /// shell subprocess, observing its real exit code — never the child's own claim about it.
    ///
    /// **G80 — every captured stream leaves this function REDACTED.** Upstream wraps each of
    /// `stdout`/`stderr` in `redactVerifyEnv(…, command.env)` before `trimOutput`
    /// (`acceptance.ts:1173-1174,1194-1195,1203-1204`), and so does this. The output of a verify
    /// command is attacker-adjacent by construction — it is whatever `cargo test`/`curl`/a build
    /// script printed, running with the orchestrator's full environment — and it lands verbatim in
    /// the acceptance ledger, which lands in a transcript. Redacting before trimming (not after) is
    /// also upstream's order and matters: a secret straddling the 12 000-char truncation point must
    /// be masked while it is still whole.
    pub async fn run_verify_command(
        command: &AcceptanceVerifyCommand,
        default_cwd: &Path,
    ) -> AcceptanceVerifyResult {
        let started = Instant::now();
        let cwd: PathBuf = resolve_verify_cwd(command, default_cwd);
        let mut cmd = super::shell_command(&command.command);
        cmd.current_dir(&cwd);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        if let Some(env) = &command.env {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let cwd_str = Some(cwd.display().to_string());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                // `child.on("error", …)` (`acceptance.ts:1198-1205`) — the error TEXT is redacted
                // too, because a spawn failure echoes the command line back (`sh: -c: …`) and a
                // verify command may legitimately carry a credential in its own argv.
                return AcceptanceVerifyResult::unmemoized(
                    command,
                    cwd_str,
                    Some(1),
                    if command.allow_failure == Some(true) {
                        VerifyRunStatus::AllowedFailure
                    } else {
                        VerifyRunStatus::Failed
                    },
                    Option::None,
                    Some(redact_verify_env(&err.to_string(), command.env.as_ref())),
                    started.elapsed().as_millis(),
                );
            }
        };

        let timeout = Duration::from_millis(command.timeout_ms.unwrap_or(DEFAULT_VERIFY_TIMEOUT_MS));

        // Never race a `self`-consuming `wait_with_output()` against the timeout: the elapsed arm
        // would drop the only `Child` handle and abandon a live process group. Drain the pipes on
        // their own tasks, keep the `Child`, and kill on expiry (`abortVerification`,
        // `acceptance.ts:1164-1178`).
        let stdout_task = child.stdout.take().map(super::spawn_pipe_drain);
        let stderr_task = child.stderr.take().map(super::spawn_pipe_drain);

        // ONE absolute deadline over exit AND output collection — see `super::drained_by` for why
        // the post-`wait()` drain must be inside it (upstream `acceptance.ts:742-759`).
        let deadline = tokio::time::Instant::now() + timeout;

        let waited = tokio::select! {
            biased;
            result = child.wait() => Some(result),
            () = tokio::time::sleep_until(deadline) => None,
        };

        let Some(waited) = waited else {
            // `abortVerification` (`acceptance.ts:1164-1178`): SIGTERM, then a hard SIGKILL a
            // second later, targeting the command's own process GROUP (it leads one — see
            // `process_group(0)` above), so the descendants the command spawned die with it.
            // `terminate_on_timeout` returns only once the process is CONFIRMED reaped.
            let _ = crate::spawn::signal::terminate_on_timeout(&mut child).await;
            // Upstream's timeout `finish(...)` still reports the output collected SO FAR
            // (`acceptance.ts:1173-1174`) — its `stdout`/`stderr` accumulators are the same ones
            // the `"close"` arm would have used. Collect the drains rather than discarding them,
            // but keep them bounded: a descendant that escaped the process group can still hold the
            // pipes open, and this arm must never be the thing that hangs.
            let (out_bytes, err_bytes) = super::drained_by(
                tokio::time::Instant::now() + super::TIMEOUT_SIGTERM_GRACE,
                stdout_task,
                stderr_task,
            )
            .await
            .unwrap_or_default();
            return AcceptanceVerifyResult::unmemoized(
                command,
                cwd_str,
                Option::None,
                VerifyRunStatus::TimedOut,
                trim_output_after(&out_bytes, command.env.as_ref()),
                // `stderr || options.abortMessage || "Acceptance verification timed out."`
                // (`acceptance.ts:1174`) — JS `||` treats an EMPTY captured stderr as absent, which
                // `trim_output_after` already spells as `None`. This crate threads no abort
                // signal/message into the runner (`input.signal`/`input.abortMessage` are unported),
                // so the literal fallback is the one that applies.
                trim_output_after(&err_bytes, command.env.as_ref())
                    .or_else(|| Some(VERIFY_TIMED_OUT_MESSAGE.to_string())),
                started.elapsed().as_millis(),
            );
        };

        // A command that EXITED while a descendant still holds its pipes is reported TIMED OUT at
        // the deadline, never awaited unbounded — upstream reaches the same verdict by the same
        // route (its `"close"` event never fires either, so only `abortVerification` resolves the
        // promise); see `super::drained_by`.
        let Some((out_bytes, err_bytes)) = super::drained_by(
            deadline + super::TIMEOUT_SIGTERM_GRACE,
            stdout_task,
            stderr_task,
        )
        .await
        else {
            return AcceptanceVerifyResult::unmemoized(
                command,
                cwd_str,
                Option::None,
                VerifyRunStatus::TimedOut,
                Option::None,
                Some(VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE.to_string()),
                started.elapsed().as_millis(),
            );
        };

        match waited {
            Ok(status_code) => {
                let exit_code = status_code.code();
                let passed = exit_code == Some(0);
                let status = if passed {
                    VerifyRunStatus::Passed
                } else if command.allow_failure == Some(true) {
                    VerifyRunStatus::AllowedFailure
                } else {
                    VerifyRunStatus::Failed
                };
                // `trimOutput(redactVerifyEnv(stdout, command.env))` / same for stderr
                // (`acceptance.ts:1194-1195`) — redact FIRST, trim second.
                AcceptanceVerifyResult::unmemoized(
                    command,
                    cwd_str,
                    exit_code,
                    status,
                    trim_output_after(&out_bytes, command.env.as_ref()),
                    trim_output_after(&err_bytes, command.env.as_ref()),
                    started.elapsed().as_millis(),
                )
            }
            Err(err) => AcceptanceVerifyResult::unmemoized(
                command,
                cwd_str,
                Some(1),
                if command.allow_failure == Some(true) {
                    VerifyRunStatus::AllowedFailure
                } else {
                    VerifyRunStatus::Failed
                },
                Option::None,
                Some(redact_verify_env(&err.to_string(), command.env.as_ref())),
                started.elapsed().as_millis(),
            ),
        }
    }

    /// `trimOutput(redactVerifyEnv(<captured bytes>, env))` (`acceptance.ts:1194-1195`) as one
    /// step, in upstream's order: the raw capture is decoded, REDACTED whole, and only then
    /// trimmed/truncated. Doing it the other way round would let the 12 000-char truncation split
    /// a secret and smuggle its prefix through.
    #[must_use]
    fn trim_output_after(
        captured: &[u8],
        env: Option<&std::collections::BTreeMap<String, String>>,
    ) -> Option<String> {
        let decoded = String::from_utf8_lossy(captured);
        trim_output(&redact_verify_env(&decoded, env))
    }

    // --------------------------------------------------------------------------------------------
    // evaluateAcceptance / acceptanceFailureMessage (acceptance.ts:769-856)
    // --------------------------------------------------------------------------------------------

    /// Input to [`evaluate_acceptance`] (`acceptance.ts:1210-1228`).
    pub struct EvaluateAcceptanceInput<'a> {
        pub acceptance: &'a ResolvedAcceptanceConfig,
        pub output: &'a str,
        pub cwd: &'a Path,
        pub report: Option<AcceptanceReport>,
        /// G79 — pi `input.fileOutput` (`acceptance.ts:1214-1220`): the content the child sent to
        /// its configured output file, searched for the acceptance report alongside `output` by
        /// [`parse_acceptance_report_sources`].
        pub file_output: Option<super::AcceptanceFileOutput<'a>>,
        pub review_result: Option<AcceptanceReviewResult>,
        /// G80 — pi `input.artifactsDir` + `input.runId` (`acceptance.ts:1226-1227`), threaded to
        /// [`run_memoized_verify_command`] (`acceptance.ts:1289-1293`). `None` disables
        /// memoization for this evaluation, which is what pi's own chain-execution group gate does
        /// (`chain-execution.ts:1037-1046,1233-1242` pass neither field).
        pub memo: Option<VerifyMemoContext<'a>>,
        /// G78 — pi `input.reportOptional` (`acceptance.ts:1225`): this caller does not insist the
        /// child emit a structured `acceptance-report` block, because it did not ASK for one.
        ///
        /// It is the same flag that suppresses the contract prompt
        /// (`formatAcceptancePrompt(acceptance, { reportOptional })` returns `""` when the policy
        /// declares neither criteria nor evidence, `acceptance.ts:409`), so leaving it `false` here
        /// while the prompt was suppressed would reject every such run for missing a report it was
        /// never told to write. See [`acceptance_requires_child_report`] and the three-arm ladder
        /// at `acceptance.ts:1251-1266`.
        pub report_optional: bool,
    }

    /// `evaluateAcceptance` (acceptance.ts:1210-1355). Async because `verified` runs REAL `verify[]`
    /// subprocesses and `checked`/above may run the REAL `git status` no-staged-files check.
    #[must_use]
    pub async fn evaluate_acceptance(input: EvaluateAcceptanceInput<'_>) -> AcceptanceLedger {
        let acceptance = input.acceptance;
        // `acceptance.ts:1230-1233` @v0.43.0: ONE `initialStatus`, written to both fields.
        let initial_status = if acceptance.level == AcceptanceLevel::None {
            AcceptanceEvidenceStatus::NotRequired
        } else {
            AcceptanceEvidenceStatus::Claimed
        };
        let mut ledger = AcceptanceLedger {
            status: initial_status.into(),
            evidence_status: initial_status,
            explicit: acceptance.explicit,
            inferred_reason: acceptance.inferred_reason.clone(),
            criteria: acceptance.criteria.iter().map(SerializableGate::from_gate).collect(),
            child_report: Option::None,
            child_report_parse_error: Option::None,
            runtime_checks: Vec::new(),
            verify_runs: Vec::new(),
            review_result: Option::None,
        };
        if acceptance.level == AcceptanceLevel::None {
            return ledger;
        }

        // `acceptance.ts:1243-1250` @v0.43.0 — a caller-supplied `report` wins outright, but is
        // RE-VALIDATED first (v0.43.0 wrapped it in `validateAcceptanceReport`; before that it was
        // trusted verbatim). Otherwise BOTH the assistant output and the child's configured output
        // file are searched, in the order `parse_acceptance_report_sources` picks (G79).
        let parsed = match input.report {
            Some(supplied) => {
                // `validateAcceptanceReport(input.report)` — upstream's default `pathLabel` is `""`
                // (`acceptance.ts:831`), which is also what every other caller in this port passes.
                let value = serde_json::to_value(&supplied).unwrap_or(Value::Null);
                let (validated, errors) = validate_acceptance_report(&value, "");
                match validated {
                    Some(report) => ParsedAcceptanceReport {
                        report: Some(report),
                        error: Option::None,
                    },
                    Option::None => ParsedAcceptanceReport {
                        report: Option::None,
                        error: Some(format!(
                            "Failed to parse acceptance-report: Invalid acceptance-report: {}",
                            errors.join("; ")
                        )),
                    },
                }
            }
            Option::None => parse_acceptance_report_sources(input.output, input.file_output.as_ref()),
        };
        // `const needsReport = acceptanceRequiresChildReport(acceptance)` (`acceptance.ts:1251`).
        let needs_report = acceptance_requires_child_report(acceptance);
        // The three-arm ladder at `acceptance.ts:1252-1266` @v0.43.0. Before this port it had only
        // two arms — report present, or reject — so a `report_optional` caller (one whose contract
        // prompt was itself suppressed, `acceptance.ts:409`) was rejected for failing to emit a
        // report it was never asked for.
        let report = parsed.report.clone();
        if let Some(report) = &report {
            ledger.child_report = Some(report.clone());
            ledger.status = AcceptanceLedgerStatus::Attested;
            ledger.evidence_status = AcceptanceEvidenceStatus::Attested;
        } else if !input.report_optional
            || needs_report
            || parsed.error.as_deref() != Some(ACCEPTANCE_REPORT_NOT_FOUND)
        {
            // A missing report still RECORDS a failed attestation check when the caller demanded
            // one, when the policy's own criteria/evidence require one, or when the report was
            // present but BROKEN (any error other than "not found").
            ledger.child_report_parse_error = parsed.error.clone();
            ledger.runtime_checks.push(AcceptanceRuntimeCheck {
                id: "attestation".to_string(),
                status: RuntimeCheckStatus::Failed,
                message: parsed
                    .error
                    .clone()
                    .unwrap_or_else(|| "Structured acceptance report missing.".to_string()),
            });
            if !input.report_optional {
                ledger.status = AcceptanceLedgerStatus::Rejected;
                ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
                return ledger;
            }
        } else {
            // `acceptance.ts:1264-1266` — the non-rejecting arm: an OPTIONAL report that is simply
            // absent from a policy that requires none. The parse error is recorded for the
            // transcript and NO runtime check is pushed, so the run keeps climbing and settles at
            // its declared level via the `claimed` fallthrough below (`:1313-1316`).
            ledger.child_report_parse_error = parsed.error.clone();
        }

        let rank = level_rank(acceptance.level).unwrap_or(0);
        let checked_rank = 2u8;
        let verified_rank = 3u8;

        // `acceptance.ts:1268-1278` @v0.43.0. NOTE the three structural changes v0.43.0 made here:
        // the rung is additionally guarded on `parsed.report` (there is nothing to check criteria
        // against without one, now that the ladder above can fall through with none), it APPENDS to
        // `runtimeChecks` rather than replacing them, and it no longer returns early on a failed
        // check — it simply declines to promote, so the `verify[]` rung below still runs and its
        // results still land on the ledger. The single rejection point is the combined check
        // further down (`:1308-1312`).
        if let Some(report) = &report
            && rank >= checked_rank
        {
            let mut checks = check_criteria_satisfied(&acceptance.criteria, report);
            checks.extend(run_structural_checks(&acceptance.evidence, report, input.cwd).await);
            ledger.runtime_checks.extend(checks);
            if !ledger
                .runtime_checks
                .iter()
                .any(|c| c.status == RuntimeCheckStatus::Failed)
            {
                ledger.status = AcceptanceLedgerStatus::Checked;
                ledger.evidence_status = AcceptanceEvidenceStatus::Checked;
            }
        }

        if rank >= verified_rank
            && (acceptance.level == AcceptanceLevel::Verified || !acceptance.verify.is_empty())
        {
            if acceptance.level == AcceptanceLevel::Verified && acceptance.verify.is_empty() {
                ledger.runtime_checks.push(AcceptanceRuntimeCheck {
                    id: "verification-config".to_string(),
                    status: RuntimeCheckStatus::Failed,
                    message: "verified acceptance requires runtime verify commands.".to_string(),
                });
                ledger.status = AcceptanceLedgerStatus::Rejected;
                ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
                return ledger;
            }
            let mut runs = Vec::new();
            for command in &acceptance.verify {
                // `runMemoizedVerifyCommand(command, input.cwd, { …, artifactsDir, runId })`
                // (`acceptance.ts:1289-1293`) — memoized when the caller supplied both, a plain
                // execution otherwise.
                runs.push(run_memoized_verify_command(command, input.cwd, input.memo).await);
            }
            ledger.verify_runs = runs;
            if ledger.verify_runs.iter().any(|run| {
                matches!(run.status, VerifyRunStatus::Failed | VerifyRunStatus::TimedOut)
            }) {
                ledger.status = AcceptanceLedgerStatus::Rejected;
                ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
                return ledger;
            }
            if !ledger
                .runtime_checks
                .iter()
                .any(|c| c.status == RuntimeCheckStatus::Failed)
            {
                ledger.status = AcceptanceLedgerStatus::Verified;
                ledger.evidence_status = AcceptanceEvidenceStatus::Verified;
            }
        }

        // `acceptance.ts:1308-1312` — the single rejection point for failed structural checks.
        if ledger
            .runtime_checks
            .iter()
            .any(|c| c.status == RuntimeCheckStatus::Failed)
        {
            ledger.status = AcceptanceLedgerStatus::Rejected;
            ledger.evidence_status = AcceptanceEvidenceStatus::Rejected;
            return ledger;
        }
        // `acceptance.ts:1313-1316` — a run that never got past `claimed` (only reachable when the
        // caller allowed a missing report) still settles at its declared level.
        if ledger.status == AcceptanceLedgerStatus::Claimed {
            let settled = match acceptance.level {
                AcceptanceLevel::Verified => AcceptanceEvidenceStatus::Verified,
                AcceptanceLevel::Checked => AcceptanceEvidenceStatus::Checked,
                AcceptanceLevel::Attested => AcceptanceEvidenceStatus::Attested,
                AcceptanceLevel::None => AcceptanceEvidenceStatus::NotRequired,
                // `auto` never survives `resolve_effective_acceptance`.
                AcceptanceLevel::Auto => AcceptanceEvidenceStatus::Claimed,
            };
            ledger.status = settled.into();
            ledger.evidence_status = settled;
        }

        // `acceptance.ts:1318-1336` @v0.43.0 — the review gate now hangs off `acceptance.review`,
        // NOT off a `level === "reviewed"` that no longer exists. Only `status` moves here;
        // `evidence_status` keeps whatever the evidence actually earned.
        if let Some(ReviewSetting::Gate(gate)) = &acceptance.review {
            match input.review_result {
                Some(review) if review.status == ReviewResultStatus::Reviewed => {
                    ledger.review_result = Some(review);
                    ledger.status = AcceptanceLedgerStatus::Reviewed;
                }
                Some(review) if review.status == ReviewResultStatus::Blockers => {
                    ledger.review_result = Some(review);
                    ledger.status = AcceptanceLedgerStatus::Rejected;
                }
                supplied => {
                    if gate.required != Some(false) {
                        ledger.review_result = Some(supplied.unwrap_or(AcceptanceReviewResult {
                            status: ReviewResultStatus::ReviewRequired,
                            findings: vec![ReviewFinding {
                                severity: ReviewFindingSeverity::NonBlocking,
                                file: Option::None,
                                issue: "Independent review has not been supplied.".to_string(),
                                rationale:
                                    "The run cannot be marked reviewed from child evidence alone."
                                        .to_string(),
                            }],
                        }));
                        ledger.status = AcceptanceLedgerStatus::ReviewRequired;
                    }
                }
            }
        }

        ledger
    }

    /// `acceptanceFailureMessage` (acceptance.ts:1357-1365).
    #[must_use]
    pub fn acceptance_failure_message(ledger: &AcceptanceLedger) -> Option<String> {
        if ledger.status != AcceptanceLedgerStatus::Rejected {
            return Option::None;
        }
        if let Some(check) = ledger
            .runtime_checks
            .iter()
            .find(|c| c.status == RuntimeCheckStatus::Failed)
        {
            return Some(format!("Acceptance rejected: {}", check.message));
        }
        if let Some(run) = ledger
            .verify_runs
            .iter()
            .find(|r| matches!(r.status, VerifyRunStatus::Failed | VerifyRunStatus::TimedOut))
        {
            let status = match run.status {
                VerifyRunStatus::Failed => "failed",
                VerifyRunStatus::TimedOut => "timed-out",
                _ => "failed",
            };
            return Some(format!("Acceptance verification '{}' {status}.", run.id));
        }
        // `acceptance.ts:1363-1364` @v0.43.0. v0.34.0 also had a `needs-parent-decision` arm; that
        // review status no longer exists, and its successor (`review-required`) is not a REJECTED
        // ledger, so it never reaches this function at all.
        match ledger.review_result.as_ref().map(|r| r.status) {
            Some(ReviewResultStatus::Blockers) => {
                Some("Acceptance review found blockers.".to_string())
            }
            _ => Some("Acceptance rejected.".to_string()),
        }
    }

    // --------------------------------------------------------------------------------------------
    // validateAcceptanceInput (acceptance.ts:138-249)
    // --------------------------------------------------------------------------------------------

    const VALID_LEVELS: &[&str] = &["auto", "none", "attested", "checked", "verified"];
    /// `EXPLICIT_REVIEWED_UNAVAILABLE` (`acceptance.ts:54` @v0.43.0) — verbatim, including the
    /// leading space the two call sites supply by interpolation (`${pathLabel} ${…}` /
    /// `${pathLabel}.level ${…}`).
    pub const EXPLICIT_REVIEWED_UNAVAILABLE: &str = "is an achieved status, not a requestable acceptance level. For a read-only reviewer call, omit acceptance. To require independent review of a writer result, use acceptance.review.required and orchestrate the reviewer separately.";
    /// `VALID_EVIDENCE_KINDS` (`acceptance.ts:36-46` @v0.43.0), in upstream's declaration order —
    /// which is load-bearing, because [`acceptance_evidence_help`] joins it verbatim into the
    /// guidance suffix v0.43.0 appends to every evidence-kind validation message.
    const VALID_EVIDENCE_KINDS: &[AcceptanceEvidenceKind] = &[
        AcceptanceEvidenceKind::ChangedFiles,
        AcceptanceEvidenceKind::TestsAdded,
        AcceptanceEvidenceKind::CommandsRun,
        AcceptanceEvidenceKind::ValidationOutput,
        AcceptanceEvidenceKind::ResidualRisks,
        AcceptanceEvidenceKind::NoStagedFiles,
        AcceptanceEvidenceKind::DiffSummary,
        AcceptanceEvidenceKind::ReviewFindings,
        AcceptanceEvidenceKind::ManualNotes,
    ];

    /// `ACCEPTANCE_OBJECT_EXAMPLE` (`acceptance.ts:49` @v0.43.0) — appended to the "must be a
    /// string level, false, or an object." message so a caller that passed the wrong SHAPE is shown
    /// a working one rather than only told it was wrong.
    const ACCEPTANCE_OBJECT_EXAMPLE: &str =
        "Example: { level: \"checked\", evidence: [\"commands-run\", \"changed-files\"] }.";

    /// `ACCEPTANCE_EVIDENCE_HELP` (`acceptance.ts:48` @v0.43.0):
    /// `Supported evidence kinds: ${VALID_EVIDENCE_KINDS.join(", ")}. ${ACCEPTANCE_OBJECT_EXAMPLE}`.
    ///
    /// Built from [`VALID_EVIDENCE_KINDS`] rather than written out, so adding an evidence kind
    /// cannot leave the help text listing a stale set — upstream derives it the same way.
    #[must_use]
    fn acceptance_evidence_help() -> String {
        let kinds: Vec<&str> = VALID_EVIDENCE_KINDS.iter().map(|k| k.as_str()).collect();
        format!(
            "Supported evidence kinds: {}. {ACCEPTANCE_OBJECT_EXAMPLE}",
            kinds.join(", ")
        )
    }

    /// `unsupportedEvidenceKindMessage` (`acceptance.ts:171-174` @v0.43.0) — v0.43.0 replaced the
    /// bare `<path> is not a supported evidence kind.` with this: it ECHOES the offending value
    /// when it was a string (`const value = typeof item === "string" ? ` "${item}"` : ""`) and
    /// appends [`acceptance_evidence_help`].
    #[must_use]
    fn unsupported_evidence_kind_message(path_label: &str, item: &Value) -> String {
        let echoed = match item.as_str() {
            Some(text) => format!(" \"{text}\""),
            Option::None => String::new(),
        };
        format!(
            "{path_label}{echoed} is not a supported evidence kind. {}",
            acceptance_evidence_help()
        )
    }

    const ACCEPTANCE_CONFIG_KEYS: &[&str] =
        &["level", "criteria", "evidence", "verify", "review", "stopRules", "reason"];
    const ACCEPTANCE_GATE_KEYS: &[&str] = &["id", "must", "evidence", "severity"];
    const ACCEPTANCE_VERIFY_KEYS: &[&str] =
        &["id", "command", "timeoutMs", "cwd", "env", "allowFailure"];
    const ACCEPTANCE_REVIEW_KEYS: &[&str] = &["agent", "focus", "required"];

    /// `validateAcceptanceInput` (acceptance.ts:138-249). `input` is the raw JSON value the caller
    /// supplied; `Value::Null` stands for pi's `undefined` (returns no errors).
    #[must_use]
    pub fn validate_acceptance_input(input: &Value, path_label: &str) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        match input {
            Value::Null => return errors,
            Value::Bool(false) => return errors,
            // `acceptance.ts:180-185` @v0.43.0. Every bare level string except `auto`, `attested`
            // and `checked` is now an error: `reviewed` is not a level at all, `none` needs a
            // reason, and `verified` needs runtime commands — which is precisely
            // `AcceptanceInput = Exclude<AcceptanceLevel, "none" | "verified"> | false |
            // AcceptanceConfig` (`shared/types.ts:684-685`) restated as messages.
            Value::String(s) => {
                if s == "reviewed" {
                    errors.push(format!("{path_label} {EXPLICIT_REVIEWED_UNAVAILABLE}"));
                } else if !VALID_LEVELS.contains(&s.as_str()) {
                    errors.push(format!("{path_label} has invalid level '{s}'."));
                } else if s == "none" {
                    errors.push(format!(
                        "{path_label} level \"none\" requires a reason; use {{ level: \"none\", reason: \"...\" }}."
                    ));
                } else if s == "verified" {
                    errors.push(format!(
                        "{path_label} level \"verified\" requires object form with at least one runtime verify command. Use level \"checked\" or provide a non-empty acceptance.verify array."
                    ));
                }
                return errors;
            }
            Value::Object(_) => {}
            _ => {
                // `acceptance.ts:188` @v0.43.0 — the message now carries
                // `ACCEPTANCE_OBJECT_EXAMPLE`.
                errors.push(format!(
                    "{path_label} must be a string level, false, or an object. {ACCEPTANCE_OBJECT_EXAMPLE}"
                ));
                return errors;
            }
        }
        let Value::Object(map) = input else {
            return errors;
        };
        for key in map.keys() {
            if !ACCEPTANCE_CONFIG_KEYS.contains(&key.as_str()) {
                errors.push(format!("{path_label}.{key} is not supported."));
            }
        }
        // `acceptance.ts:195-199` @v0.43.0.
        if map.get("level").and_then(Value::as_str) == Some("reviewed") {
            errors.push(format!("{path_label}.level {EXPLICIT_REVIEWED_UNAVAILABLE}"));
        } else if let Some(level) = map.get("level")
            && !level.as_str().is_some_and(|l| VALID_LEVELS.contains(&l)) {
                errors.push(format!(
                    "{path_label}.level must be one of auto, none, attested, checked, verified."
                ));
            }
        if map.get("level").and_then(Value::as_str) == Some("none")
            && map
                .get("reason")
                .and_then(Value::as_str).is_none_or(|r| r.trim().is_empty())
        {
            errors.push(format!("{path_label}.reason is required when level is none."));
        }
        if let Some(reason) = map.get("reason")
            && !reason.is_string() {
                errors.push(format!("{path_label}.reason must be a string."));
            }
        validate_criteria_input(&mut errors, map.get("criteria"), path_label);
        // `acceptance.ts:239-247` @v0.43.0 — both arms now carry the guidance suffix.
        match map.get("evidence") {
            Some(Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    if item
                        .as_str()
                        .and_then(AcceptanceEvidenceKind::from_wire).is_none()
                    {
                        errors.push(unsupported_evidence_kind_message(
                            &format!("{path_label}.evidence[{index}]"),
                            item,
                        ));
                    }
                }
            }
            Some(_) => errors.push(format!(
                "{path_label}.evidence must be an array. {}",
                acceptance_evidence_help()
            )),
            Option::None => {}
        }
        // `acceptance.ts:248-252` @v0.43.0: an object-form `verified` policy MUST carry at least one
        // runtime command — the level's whole meaning is "the orchestrator ran something", so a
        // command-less `verified` could only ever have been satisfied by the child's own claim.
        let verified_without_commands = map.get("level").and_then(Value::as_str) == Some("verified")
            && !map
                .get("verify")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty());
        if verified_without_commands {
            errors.push(format!(
                "{path_label}.verify must contain at least one runtime command when level is verified. Use level \"checked\" or provide a non-empty acceptance.verify array."
            ));
        }
        // Upstream's `else if` suppresses only the generic "must be an array" message; the
        // per-command checks below it are an unconditional `if (Array.isArray(value.verify))`.
        validate_verify_input(
            &mut errors,
            map.get("verify"),
            path_label,
            !verified_without_commands,
        );
        validate_review_input(&mut errors, map.get("review"), path_label);
        match map.get("stopRules") {
            Some(Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    if !item.is_string() {
                        errors.push(format!("{path_label}.stopRules[{index}] must be a string."));
                    }
                }
            }
            Some(_) => errors.push(format!("{path_label}.stopRules must be an array.")),
            Option::None => {}
        }
        errors
    }

    /// `validateAcceptanceInput`'s `criteria` block (`acceptance.ts:204-238` @v0.43.0).
    ///
    /// v0.43.0 added the `criterionIds` set (`:206,218-224`): two gates whose ids differ only in
    /// case, whitespace or `_`-vs-`-` spelling collapse onto ONE
    /// [`normalized_token`] — which is the key
    /// [`check_criteria_satisfied`] matches the child's reported criteria against — so the second
    /// gate would silently shadow the first and its `must` would never be enforced. The check is
    /// deliberately inside the `else` of the `id is required` test: a blank/absent id has already
    /// been reported and must not additionally claim to duplicate the empty token.
    fn validate_criteria_input(errors: &mut Vec<String>, criteria: Option<&Value>, path_label: &str) {
        match criteria {
            Option::None => {}
            Some(Value::Array(items)) => {
                // `const criterionIds = new Set<string>()` (`acceptance.ts:206`) — scoped to ONE
                // `criteria[]` array, so two different policies may reuse an id freely.
                let mut criterion_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for (index, criterion) in items.iter().enumerate() {
                    if criterion.is_string() {
                        continue;
                    }
                    let cpath = format!("{path_label}.criteria[{index}]");
                    let Value::Object(gate) = criterion else {
                        errors.push(format!("{cpath} must be a string or an object."));
                        continue;
                    };
                    for key in gate.keys() {
                        if !ACCEPTANCE_GATE_KEYS.contains(&key.as_str()) {
                            errors.push(format!("{cpath}.{key} is not supported."));
                        }
                    }
                    // `acceptance.ts:218-224` @v0.43.0.
                    match gate.get("id").and_then(Value::as_str).filter(|i| !i.trim().is_empty()) {
                        Option::None => errors.push(format!("{cpath}.id is required.")),
                        Some(id) => {
                            let normalized_id = normalized_token(id);
                            if criterion_ids.contains(&normalized_id) {
                                errors.push(format!(
                                    "{cpath}.id duplicates normalized criterion id '{normalized_id}'."
                                ));
                            }
                            criterion_ids.insert(normalized_id);
                        }
                    }
                    if gate.get("must").and_then(Value::as_str).is_none_or(|m| m.trim().is_empty()) {
                        errors.push(format!("{cpath}.must is required."));
                    }
                    // `acceptance.ts:226-233` @v0.43.0 — both arms carry the guidance suffix.
                    match gate.get("evidence") {
                        Some(Value::Array(evs)) => {
                            for (ei, item) in evs.iter().enumerate() {
                                if item.as_str().and_then(AcceptanceEvidenceKind::from_wire).is_none() {
                                    errors.push(unsupported_evidence_kind_message(
                                        &format!("{cpath}.evidence[{ei}]"),
                                        item,
                                    ));
                                }
                            }
                        }
                        Some(_) => errors.push(format!(
                            "{cpath}.evidence must be an array. {}",
                            acceptance_evidence_help()
                        )),
                        Option::None => {}
                    }
                    if let Some(sev) = gate.get("severity")
                        && !matches!(sev.as_str(), Some("required") | Some("recommended")) {
                            errors.push(format!("{cpath}.severity must be required or recommended."));
                        }
                }
            }
            Some(_) => errors.push(format!("{path_label}.criteria must be an array.")),
        }
    }

    /// `report_shape_errors` is upstream's `else if` guard (`acceptance.ts:250-252`): `false`
    /// suppresses the generic `.verify must be an array.` message because the caller already
    /// emitted the more specific `verified`-needs-commands one.
    fn validate_verify_input(
        errors: &mut Vec<String>,
        verify: Option<&Value>,
        path_label: &str,
        report_shape_errors: bool,
    ) {
        match verify {
            Option::None => {}
            Some(Value::Array(items)) => {
                for (index, command) in items.iter().enumerate() {
                    let vpath = format!("{path_label}.verify[{index}]");
                    let Value::Object(cmd) = command else {
                        errors.push(format!("{vpath} must be an object."));
                        continue;
                    };
                    for key in cmd.keys() {
                        if !ACCEPTANCE_VERIFY_KEYS.contains(&key.as_str()) {
                            errors.push(format!("{vpath}.{key} is not supported."));
                        }
                    }
                    if cmd.get("id").and_then(Value::as_str).is_none_or(|i| i.trim().is_empty()) {
                        errors.push(format!("{vpath}.id is required."));
                    }
                    if cmd.get("command").and_then(Value::as_str).is_none_or(|c| c.trim().is_empty()) {
                        errors.push(format!("{vpath}.command is required."));
                    }
                    if let Some(timeout) = cmd.get("timeoutMs") {
                        let ok = timeout
                            .as_i64()
                            .filter(|_| timeout.as_f64().is_some_and(|f| f.fract() == 0.0))
                            .is_some_and(|v| v >= 1);
                        if !ok {
                            errors.push(format!("{vpath}.timeoutMs must be an integer >= 1."));
                        }
                    }
                    if let Some(cwd) = cmd.get("cwd")
                        && !cwd.is_string() {
                            errors.push(format!("{vpath}.cwd must be a string."));
                        }
                    match cmd.get("env") {
                        Some(Value::Object(env)) => {
                            for (env_key, env_value) in env {
                                if !env_value.is_string() {
                                    errors.push(format!("{vpath}.env.{env_key} must be a string."));
                                }
                            }
                        }
                        Some(_) => errors.push(format!("{vpath}.env must be an object.")),
                        Option::None => {}
                    }
                    if let Some(allow) = cmd.get("allowFailure")
                        && !allow.is_boolean() {
                            errors.push(format!("{vpath}.allowFailure must be a boolean."));
                        }
                }
            }
            Some(_) if report_shape_errors => {
                errors.push(format!("{path_label}.verify must be an array."));
            }
            Some(_) => {}
        }
    }

    fn validate_review_input(errors: &mut Vec<String>, review: Option<&Value>, path_label: &str) {
        match review {
            Option::None | Some(Value::Bool(false)) => {}
            Some(Value::Object(map)) => {
                for key in map.keys() {
                    if !ACCEPTANCE_REVIEW_KEYS.contains(&key.as_str()) {
                        errors.push(format!("{path_label}.review.{key} is not supported."));
                    }
                }
                if let Some(agent) = map.get("agent")
                    && !agent.is_string() {
                        errors.push(format!("{path_label}.review.agent must be a string."));
                    }
                if let Some(focus) = map.get("focus")
                    && !focus.is_string() {
                        errors.push(format!("{path_label}.review.focus must be a string."));
                    }
                if let Some(required) = map.get("required")
                    && !required.is_boolean() {
                        errors.push(format!("{path_label}.review.required must be a boolean."));
                    }
            }
            Some(_) => errors.push(format!("{path_label}.review must be false or an object.")),
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

        use super::*;
        use serde_json::json;

        fn cfg(config: AcceptanceConfig) -> Option<AcceptanceInput> {
            Some(AcceptanceInput::Config(config))
        }

        fn resolve(input: AcceptanceResolveInput) -> ResolvedAcceptanceConfig {
            resolve_effective_acceptance(&input)
        }

        fn report_value(overrides: Value) -> Value {
            let mut base = json!({
                "criteriaSatisfied": [{"id": "criterion-1", "status": "satisfied", "evidence": "verified in test"}],
                "changedFiles": ["src/file.ts"],
                "testsAddedOrUpdated": ["test/file.test.ts"],
                "commandsRun": [{"command": "npm test", "result": "passed", "summary": "passed"}],
                "validationOutput": ["tests passed"],
                "residualRisks": [],
                "noStagedFiles": true,
                "notes": "complete"
            });
            if let (Value::Object(b), Value::Object(o)) = (&mut base, overrides) {
                for (k, v) in o {
                    b.insert(k, v);
                }
            }
            base
        }

        fn report_text(overrides: Value, fence: &str) -> String {
            format!(
                "done\n```{fence}\n{}\n```",
                serde_json::to_string(&report_value(overrides)).unwrap()
            )
        }

        fn temp_dir() -> tempfile::TempDir {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(dir.path().join("file.txt"), "hello\n").expect("seed");
            dir
        }

        // ---- inferLevel / resolveEffectiveAcceptance ----

        #[test]
        fn infers_policies_for_reviewer_writer_async_and_dynamic() {
            assert_eq!(
                resolve(AcceptanceResolveInput {
                    agent_name: "reviewer".into(),
                    task: Some("Review-only. Do not edit.".into()),
                    mode: Some(SubagentRunMode::Single),
                    ..Default::default()
                })
                .level,
                AcceptanceLevel::Attested
            );
            assert_eq!(
                resolve(AcceptanceResolveInput {
                    agent_name: "worker".into(),
                    task: Some("Implement the fix".into()),
                    mode: Some(SubagentRunMode::Single),
                    ..Default::default()
                })
                .level,
                AcceptanceLevel::Checked
            );
            // `acceptance.ts:111-121` @v0.43.0 — the risky branch resolves to `checked` (v0.34.0
            // said `reviewed`, a level that no longer exists) and expresses "an independent
            // reviewer must sign this off" through the REQUIRED review gate instead. Both halves
            // are asserted, so a regression that drops the gate cannot hide behind the level.
            let async_write = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement the fix".into()),
                is_async: true,
                ..Default::default()
            });
            assert_eq!(async_write.level, AcceptanceLevel::Checked);
            assert_eq!(
                async_write.review,
                Some(ReviewSetting::Gate(AcceptanceReviewGate {
                    agent: Some("reviewer".into()),
                    focus: None,
                    required: Some(true),
                }))
            );
            assert_eq!(
                async_write.evidence,
                required_evidence_for_level(AcceptanceLevel::Checked)
            );
            let dynamic = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Fix each item".into()),
                mode: Some(SubagentRunMode::Chain),
                dynamic: true,
                ..Default::default()
            });
            assert_eq!(dynamic.level, AcceptanceLevel::Checked);
            assert_eq!(
                dynamic.review,
                Some(ReviewSetting::Gate(AcceptanceReviewGate {
                    agent: Some("reviewer".into()),
                    focus: None,
                    required: Some(true),
                }))
            );
        }

        #[test]
        fn explicit_acceptance_strengthens_inferred_policy() {
            let resolved = resolve(AcceptanceResolveInput {
                agent_name: "reviewer".into(),
                task: Some("Review-only.".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Verified),
                    verify: Some(vec![AcceptanceVerifyCommand {
                        id: "ok".into(),
                        command: "node --version".into(),
                        timeout_ms: None,
                        cwd: None,
                        env: None,
                        allow_failure: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(resolved.level, AcceptanceLevel::Verified);
            assert_eq!(resolved.verify.first().map(|v| v.id.as_str()), Some("ok"));
        }

        #[test]
        fn explicit_none_with_reason_disables_inferred_gates() {
            let resolved = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::None),
                    reason: Some("parent is doing manual acceptance".into()),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(resolved.level, AcceptanceLevel::None);
            assert!(resolved.evidence.is_empty());
        }

        // ---- formatAcceptancePrompt ----

        #[test]
        fn formats_a_standardized_child_prompt_section() {
            let resolved = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    criteria: Some(vec![CriterionInput::Text("Patch the bug".into())]),
                    stop_rules: Some(vec!["Do not stop after analysis".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let prompt = format_acceptance_prompt(&resolved, false);
            assert!(prompt.contains("## Acceptance Contract"));
            assert!(prompt.contains("Acceptance level: checked"));
            assert!(prompt.contains("Patch the bug"));
            assert!(prompt.contains("```acceptance-report"));
            assert!(prompt.contains("array fields contain strings"));
            assert!(prompt.contains("\"reviewFindings\": [\n    \"blocker:"));
        }

        // ---- parseAcceptanceReport / stripAcceptanceReport ----

        #[test]
        fn parses_acceptance_report_fences_and_ignores_unrelated_json() {
            let parsed = parse_acceptance_report(&report_text(json!({}), "acceptance-report"));
            let report = parsed.report.expect("report");
            assert_eq!(report.changed_files.as_deref(), Some(&["src/file.ts".to_string()][..]));
            assert!(parsed.error.is_none());

            let generic = parse_acceptance_report("done\n```json\n{\"notes\":\"not an acceptance report\"}\n```");
            assert!(generic.report.is_none());
            assert!(generic.error.as_deref().unwrap().contains("Structured acceptance report not found"));

            let criteria_only = parse_acceptance_report(
                "done\n```json\n{\"criteriaSatisfied\":[{\"id\":\"criterion-1\",\"status\":\"satisfied\",\"evidence\":\"example\"}]}\n```",
            );
            assert!(criteria_only.report.is_none());

            let malformed = parse_acceptance_report("```acceptance-report\n{bad-json\n```");
            assert!(malformed.report.is_none());
            assert!(malformed.error.as_deref().unwrap().contains("Failed to parse acceptance-report"));
        }

        #[test]
        fn parses_reports_from_json_family_fences_and_strips_them() {
            for fence in ["json", "jsonc", "json5"] {
                let output = report_text(json!({}), fence);
                let parsed = parse_acceptance_report(&output);
                assert!(parsed.report.is_some(), "fence {fence}");
                assert_eq!(strip_acceptance_report(&output), "done");
            }
        }

        #[test]
        fn strips_trailing_json_report_after_earlier_unrelated_json_fence() {
            let report_json = serde_json::to_string(&report_value(json!({}))).unwrap();
            let output = format!(
                "metadata\n```json\n{}\n```\ndone\n```json\n{}\n```",
                serde_json::to_string(&json!({"notes": "not an acceptance report"})).unwrap(),
                report_json,
            );
            let parsed = parse_acceptance_report(&output);
            assert!(parsed.report.is_some());
            let expected = format!(
                "metadata\n```json\n{}\n```\ndone",
                serde_json::to_string(&json!({"notes": "not an acceptance report"})).unwrap()
            );
            assert_eq!(strip_acceptance_report(&output), expected);
        }

        #[test]
        fn unwraps_acceptance_report_wrapper_objects() {
            let wrapped = serde_json::to_string(&json!({"acceptance-report": report_value(json!({}))})).unwrap();
            let output = format!("done\n```json\n{wrapped}\n```");
            let parsed = parse_acceptance_report(&output);
            let report = parsed.report.expect("report");
            assert_eq!(
                report.tests_added_or_updated.as_deref(),
                Some(&["test/file.test.ts".to_string()][..])
            );
            assert_eq!(strip_acceptance_report(&output), "done");
        }

        #[test]
        fn report_shaped_generic_json_without_criteria_is_not_stripped() {
            let output = "done\n```json\n{\"changedFiles\":[\"src/file.ts\"]}\n```".to_string();
            assert!(parse_acceptance_report(&output).report.is_none());
            assert_eq!(strip_acceptance_report(&output), output);
        }

        #[test]
        fn reports_field_level_validation_errors() {
            let bad_review = parse_acceptance_report(&report_text(
                json!({"reviewFindings": [{"id": "B-1", "severity": "blocker", "finding": "Missing evidence"}]}),
                "acceptance-report",
            ));
            assert!(bad_review.report.is_none());
            // G79 — `validateStringArrayField` now demands a NON-EMPTY string (`acceptance.ts:827`).
            assert!(bad_review.error.as_deref().unwrap().contains("reviewFindings[0]: expected non-empty string; got object"));

            let bad_command = parse_acceptance_report(&report_text(
                json!({"commandsRun": [{"command": "npm test", "exitCode": 0}]}),
                "acceptance-report",
            ));
            let err = bad_command.error.as_deref().unwrap();
            assert!(err.contains("commandsRun[0].result: expected one of \"passed\", \"failed\", \"not-run\"; got missing"));
            assert!(err.contains("commandsRun[0].summary: expected non-empty string; got missing"));

            // G79 — `"done"` is now a recognized ALIAS of `satisfied` (`acceptance.ts:520`), so the
            // unrecognized-status assertion uses a token that really is unrecognized. The alias
            // itself is asserted separately in
            // `normalizes_status_aliases_field_aliases_and_singleton_shapes`.
            let bad_criteria = parse_acceptance_report(&report_text(
                json!({"criteriaSatisfied": [{"id": 7, "status": "maybe", "evidence": ""}]}),
                "acceptance-report",
            ));
            let err = bad_criteria.error.as_deref().unwrap();
            assert!(err.contains("criteriaSatisfied[0].id: expected string; got number 7"));
            assert!(err.contains("criteriaSatisfied[0].status: expected one of \"satisfied\", \"not-satisfied\", \"not-applicable\"; got \"maybe\""));
            assert!(err.contains("criteriaSatisfied[0].evidence: expected non-empty string; got \"\""));
        }

        // ---- G79: report normalization, recovery and sources (acceptance.ts:484-772) ----

        #[test]
        fn normalizes_status_aliases_field_aliases_and_singleton_shapes() {
            // Every one of these shapes was REJECTED before v0.43.0's normalizer: snake_case keys
            // were "unsupported", a lone object where an array belongs failed the array check, a
            // bare string where a `string[]` belongs failed `string[]`, `"true"` failed the boolean
            // check, and `Done`/`OK` failed the status/result enums.
            let parsed = parse_acceptance_report(
                r#"done
```acceptance-report
{
  "criteria_satisfied": {"id": "C 1", "status": "Done", "evidence": "did it"},
  "changed_files": "src/file.rs",
  "tests_added_or_updated": ["tests/file.rs"],
  "commands_run": {"command": "cargo test", "result": "OK", "summary": "green"},
  "validation_output": "all green",
  "residual_risks": [],
  "no_staged_files": "true",
  "manual_notes": "nothing else"
}
```"#,
            );
            assert_eq!(parsed.error, None);
            let report = parsed.report.expect("the normalized report parses");
            let criterion = &report.criteria_satisfied.as_ref().unwrap()[0];
            assert_eq!(criterion.id.as_deref(), Some("c-1"));
            assert_eq!(criterion.status, CriterionStatus::Satisfied);
            assert_eq!(
                report.changed_files.as_deref(),
                Some(["src/file.rs".to_string()].as_slice())
            );
            assert_eq!(
                report.validation_output.as_deref(),
                Some(["all green".to_string()].as_slice())
            );
            let command = &report.commands_run.as_ref().unwrap()[0];
            assert_eq!(command.result, CommandRunResult::Passed);
            assert_eq!(report.no_staged_files, Some(true));
            assert_eq!(report.manual_notes.as_deref(), Some("nothing else"));
        }

        #[test]
        fn rejects_duplicate_normalized_criterion_ids_and_unsupported_fields() {
            let parsed = parse_acceptance_report(&report_text(
                json!({"criteriaSatisfied": [
                    {"id": "c 1", "status": "satisfied", "evidence": "one"},
                    {"id": "C_1", "status": "satisfied", "evidence": "two"}
                ]}),
                "acceptance-report",
            ));
            let err = parsed.error.as_deref().expect("duplicate ids are an error");
            assert!(
                err.contains("criteriaSatisfied[1].id: duplicate normalized criterion id 'c-1'"),
                "{err}"
            );

            let unsupported = parse_acceptance_report(&report_text(
                json!({"criteriaSatisfied": [
                    {"id": "c1", "status": "satisfied", "evidence": "one", "confidence": 0.9}
                ]}),
                "acceptance-report",
            ));
            let err = unsupported.error.as_deref().expect("unknown fields are an error");
            assert!(
                err.contains("criteriaSatisfied[0].confidence: unsupported acceptance criterion field"),
                "{err}"
            );

            let stray = parse_acceptance_report(&report_text(
                json!({"totallyUnknown": 1}),
                "acceptance-report",
            ));
            let err = stray.error.as_deref().expect("unknown report fields are an error");
            assert!(
                err.contains("totallyUnknown: unsupported acceptance report field"),
                "{err}"
            );
        }

        #[test]
        fn unwraps_every_wrapper_spelling_and_flags_siblings() {
            for wrapper in ["acceptance", "acceptance-report", "acceptance_report", "acceptanceReport"] {
                let mut map = serde_json::Map::new();
                map.insert(wrapper.to_string(), report_value(json!({})));
                let body = Value::Object(map);
                let text = format!(
                    "done\n```acceptance-report\n{}\n```",
                    serde_json::to_string(&body).unwrap()
                );
                let parsed = parse_acceptance_report(&text);
                assert_eq!(parsed.error, None, "wrapper `{wrapper}` must unwrap");
                assert!(parsed.report.is_some(), "wrapper `{wrapper}` must unwrap");
            }

            let sibling = json!({"acceptance": report_value(json!({})), "extra": 1});
            let parsed = parse_acceptance_report(&format!(
                "done\n```acceptance-report\n{}\n```",
                serde_json::to_string(&sibling).unwrap()
            ));
            let err = parsed.error.as_deref().expect("a sibling key is an error");
            assert!(
                err.contains("extra: unsupported alongside acceptance report wrapper 'acceptance'"),
                "{err}"
            );

            let ambiguous = json!({
                "acceptance": report_value(json!({})),
                "acceptanceReport": report_value(json!({})),
            });
            let parsed = parse_acceptance_report(&format!(
                "done\n```acceptance-report\n{}\n```",
                serde_json::to_string(&ambiguous).unwrap()
            ));
            let err = parsed.error.as_deref().expect("two wrappers are ambiguous");
            assert!(
                err.contains("multiple acceptance report wrappers are ambiguous"),
                "{err}"
            );
        }

        #[test]
        fn recovers_an_unterminated_acceptance_report_fence() {
            let body = serde_json::to_string(&report_value(json!({}))).unwrap();
            let recovered = parse_acceptance_report(&format!("done\n```acceptance-report\n{body}"));
            assert_eq!(recovered.error, None);
            assert!(recovered.report.is_some());

            // The underscore spelling is accepted everywhere the hyphenated one is
            // (`acceptance.ts:702-703`).
            let underscored = parse_acceptance_report(&format!("done\n```acceptance_report\n{body}\n```"));
            assert_eq!(underscored.error, None);
            assert!(underscored.report.is_some());

            // An opened fence whose body is not JSON is reported as a defect of THIS report and is
            // never allowed to fall through to the generic-JSON or marker paths.
            let broken = parse_acceptance_report("done\n```acceptance-report\nnot json at all");
            assert!(broken.report.is_none());
            assert!(
                broken
                    .error
                    .as_deref()
                    .unwrap()
                    .starts_with("Failed to parse acceptance-report:"),
                "{:?}",
                broken.error
            );

            let empty = parse_acceptance_report("done\n```acceptance-report\n\n```");
            assert_eq!(
                empty.error.as_deref(),
                Some("Failed to parse acceptance-report: Empty or unterminated acceptance-report fence.")
            );
        }

        /// A model cut off mid-opener — `"…\n```acceptance-report"` with no newline after the tag —
        /// must still be reported as a FENCE DEFECT, not as "no report at all".
        ///
        /// `acceptance.ts:702`'s guard is `/```acceptance[-_]report\b/i.test(output)`: tag presence,
        /// with no `[^\n]*\n` anchor. Reusing the offset-finding opener helper (which needs the
        /// newline, per `acceptance.ts:671`) collapsed this case onto
        /// [`ACCEPTANCE_REPORT_NOT_FOUND`] — and that constant is the single discriminator both
        /// [`parse_acceptance_report_sources`] and [`super::select_acceptance_report_source`] use to
        /// decide a report is genuinely ABSENT and the other source may be consulted. A truncated
        /// `file-only` artifact would therefore have been silently replaced by the assistant text.
        #[test]
        fn a_fence_opener_with_no_trailing_newline_is_a_defect_not_an_absent_report() {
            for opener in [
                "done\n```acceptance-report",
                "done\n```acceptance_report",
                // Trailing info-string content, still with no newline (`\b` then `[^\n]*`).
                "done\n```acceptance-report ",
            ] {
                let parsed = parse_acceptance_report(opener);
                assert!(parsed.report.is_none(), "{opener:?} -> {:?}", parsed.report);
                assert_eq!(
                    parsed.error.as_deref(),
                    Some(
                        "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
                    ),
                    "{opener:?} must report the fence defect, never {ACCEPTANCE_REPORT_NOT_FOUND:?}"
                );
            }

            // `\b` still bites: a tag that runs straight into another word character is NOT an
            // acceptance-report fence, so this one genuinely IS absent.
            let unrelated = parse_acceptance_report("done\n```acceptance-reporting");
            assert_eq!(unrelated.error.as_deref(), Some(ACCEPTANCE_REPORT_NOT_FOUND));

            // And the load-bearing consequence: an absent report falls through to the other
            // source, but this defect must NOT — it is surfaced verbatim.
            let file = super::super::AcceptanceFileOutput {
                content: "",
                path: std::path::Path::new("out.md"),
                authoritative: false,
            };
            let sources =
                parse_acceptance_report_sources("done\n```acceptance-report", Some(&file));
            assert!(sources.report.is_none());
            assert_eq!(
                sources.error.as_deref(),
                Some(
                    "Failed to parse acceptance-report: Empty or unterminated acceptance-report fence."
                )
            );
        }

        #[test]
        fn report_shaped_generic_json_surfaces_its_validation_errors() {
            // v0.34.0 swallowed this: a `json` fence that is unmistakably a report but has one bad
            // field read as unrelated prose, so the run silently had "no report".
            let text = format!(
                "prose\n```json\n{}\n```",
                serde_json::to_string(&json!({
                    "criteriaSatisfied": [{"id": "c1", "status": "maybe", "evidence": "x"}],
                    "changedFiles": ["a.rs"],
                }))
                .unwrap()
            );
            let parsed = parse_acceptance_report(&text);
            assert!(parsed.report.is_none());
            let err = parsed.error.as_deref().expect("a report-shaped json fence reports errors");
            assert!(err.starts_with("Failed to parse acceptance-report: Invalid acceptance-report:"), "{err}");

            // Genuinely unrelated JSON stays quiet.
            let unrelated = parse_acceptance_report("prose\n```json\n{\"hello\": \"world\"}\n```");
            assert_eq!(unrelated.error.as_deref(), Some(ACCEPTANCE_REPORT_NOT_FOUND));
        }

        #[test]
        fn marker_path_distinguishes_missing_from_unterminated_objects() {
            assert_eq!(
                parse_acceptance_report("ACCEPTANCE_REPORT: nope").error.as_deref(),
                Some("Failed to parse acceptance-report: Expected a JSON object after ACCEPTANCE_REPORT:.")
            );
            assert_eq!(
                parse_acceptance_report("ACCEPTANCE_REPORT: {\"changedFiles\": [")
                    .error
                    .as_deref(),
                Some("Failed to parse acceptance-report: Unterminated JSON object after ACCEPTANCE_REPORT:.")
            );
        }

        #[test]
        fn report_sources_prefer_the_file_when_authoritative_and_never_paper_over_a_defect() {
            let good = format!(
                "done\n```acceptance-report\n{}\n```",
                serde_json::to_string(&report_value(json!({"diffSummary": "from-file"}))).unwrap()
            );
            let path = std::path::Path::new("out.md");

            // Not authoritative: the assistant output is primary, the file is the fallback.
            let from_file_fallback = parse_acceptance_report_sources(
                "no report here",
                Some(&super::super::AcceptanceFileOutput {
                    content: &good,
                    path,
                    authoritative: false,
                }),
            );
            assert_eq!(
                from_file_fallback
                    .report
                    .as_ref()
                    .and_then(|r| r.diff_summary.as_deref()),
                Some("from-file")
            );

            // Authoritative (`outputMode: "file-only"`): the file is searched FIRST and wins even
            // when the assistant output also carries a report.
            let assistant = format!(
                "done\n```acceptance-report\n{}\n```",
                serde_json::to_string(&report_value(json!({"diffSummary": "from-text"}))).unwrap()
            );
            let file_first = parse_acceptance_report_sources(
                &assistant,
                Some(&super::super::AcceptanceFileOutput {
                    content: &good,
                    path,
                    authoritative: true,
                }),
            );
            assert_eq!(
                file_first
                    .report
                    .as_ref()
                    .and_then(|r| r.diff_summary.as_deref()),
                Some("from-file")
            );

            // A MALFORMED report in the primary source is surfaced, never papered over with the
            // secondary (`acceptance.ts:767-771`) — and the file's parse errors carry the path.
            let malformed = "done\n```acceptance-report\n{\"criteriaSatisfied\": [{\"id\": \"c1\"}]}\n```";
            let primary_defect = parse_acceptance_report_sources(
                malformed,
                Some(&super::super::AcceptanceFileOutput {
                    content: &good,
                    path,
                    authoritative: false,
                }),
            );
            assert!(primary_defect.report.is_none());
            assert!(
                primary_defect.error.as_deref().unwrap().contains("Invalid acceptance-report"),
                "{:?}",
                primary_defect.error
            );

            let file_defect = parse_acceptance_report_sources(
                "no report here",
                Some(&super::super::AcceptanceFileOutput {
                    content: malformed,
                    path,
                    authoritative: false,
                }),
            );
            assert!(
                file_defect
                    .error
                    .as_deref()
                    .unwrap()
                    .ends_with("(in configured output out.md)"),
                "{:?}",
                file_defect.error
            );
        }

        // ---- runVerifyCommand (acceptance.ts:713-767) — the SECOND copy of the runner ----

        /// SUBA-027 regression, mirror of
        /// `super::super::tests::a_verify_command_that_daemonizes_still_returns_within_its_timeout`.
        ///
        /// This module is the second copy of the verify runner, so it carried the identical
        /// unbounded post-`wait()` drain and the identical hang. Both copies now share
        /// [`super::super::drained_by`], and this test is what keeps them from drifting apart
        /// again: a command that exits 0 while a backgrounded descendant still holds its
        /// stdout/stderr must settle as [`VerifyRunStatus::TimedOut`] on its own deadline, exactly
        /// as upstream's `abortVerification` → `finish({status: "timed-out", …})` does
        /// (`acceptance.ts:742-759` @v0.34.0).
        #[cfg(unix)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_daemonizing_verify_command_times_out_instead_of_hanging_the_model_copy() {
            let dir = temp_dir();
            let command = AcceptanceVerifyCommand {
                id: "daemonizes".into(),
                command: "sleep 300 & echo $! > descendant; exit 0".into(),
                timeout_ms: Some(200),
                cwd: Option::None,
                env: Option::None,
                allow_failure: Option::None,
            };

            let started = std::time::Instant::now();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                run_verify_command(&command, dir.path()),
            )
            .await
            .expect(
                "run_verify_command must honor timeoutMs even when a backgrounded grandchild \
                 still holds the stdout/stderr pipe",
            );

            assert_eq!(
                result.status,
                VerifyRunStatus::TimedOut,
                "upstream resolves this shape through abortVerification's finish(), i.e. as \
                 timed-out, never as a pass"
            );
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "the call must return on its own deadline, not the descendant's lifetime, got {:?}",
                started.elapsed()
            );

            // Clean up the deliberately-daemonised descendant this test created.
            let pid_path = dir.path().join("descendant");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if let Ok(raw) = std::fs::read_to_string(&pid_path)
                    && let Ok(pid) = raw.trim().parse::<u32>()
                {
                    let _ = std::process::Command::new("kill")
                        .args(["-KILL", &pid.to_string()])
                        .status();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }

        // ---- evaluateAcceptance (async, real subprocess / real git) ----

        // ---- G78: the reportOptional ladder (`acceptance.ts:1251-1266` @v0.43.0) ----

        /// A policy declaring NO criteria and NO evidence — `acceptanceRequiresChildReport` is
        /// `false` for it (`acceptance.ts:403-405`), so a `report_optional` caller is one that
        /// never showed the child a contract block at all (`:409`).
        fn attested_policy_requiring_no_report() -> ResolvedAcceptanceConfig {
            // Built directly rather than through `resolve_effective_acceptance`, which always
            // merges `requiredEvidenceForLevel` and the inferred criteria in
            // (`acceptance.ts:283-292`) and so can never yield this shape. Upstream reaches it via
            // an agent contract, whose own resolution supplies neither.
            let acceptance = ResolvedAcceptanceConfig {
                level: AcceptanceLevel::Attested,
                explicit: true,
                inferred_reason: Vec::new(),
                criteria: Vec::new(),
                evidence: Vec::new(),
                verify: Vec::new(),
                review: Option::None,
                stop_rules: Vec::new(),
                reason: Option::None,
            };
            assert!(
                !acceptance_requires_child_report(&acceptance),
                "premise: this policy must require no child report, got {acceptance:?}"
            );
            acceptance
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn report_optional_does_not_reject_a_merely_absent_report() {
            // `acceptance.ts:1264-1266` — the third arm: no runtime check is pushed and the run
            // keeps climbing, settling at its declared level via `:1313-1316`.
            let dir = temp_dir();
            let acceptance = attested_policy_requiring_no_report();
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "just prose, no report block",
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: true,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Attested);
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Attested);
            assert!(
                ledger.runtime_checks.is_empty(),
                "no attestation check may be recorded on this arm: {:?}",
                ledger.runtime_checks
            );
            assert_eq!(
                ledger.child_report_parse_error.as_deref(),
                Some(ACCEPTANCE_REPORT_NOT_FOUND),
                "the absence is still RECORDED for the transcript"
            );
            assert!(acceptance_failure_message(&ledger).is_none());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_same_absent_report_still_rejects_when_the_caller_is_not_report_optional() {
            // The control for the test above: only `report_optional` changes.
            let dir = temp_dir();
            let acceptance = attested_policy_requiring_no_report();
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "just prose, no report block",
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Rejected);
            assert_eq!(ledger.runtime_checks.len(), 1);
            assert_eq!(ledger.runtime_checks[0].id, "attestation");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn report_optional_still_records_a_check_when_the_policy_needs_a_report() {
            // `needsReport` (`acceptance.ts:1251,1256`): a policy WITH criteria demands one however
            // optional the caller thinks the report is — the check is pushed, the early return is
            // skipped, and the combined check at `:1308-1312` is what rejects.
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    criteria: Some(vec![CriterionInput::Gate(AcceptanceGate {
                        id: Some("regression".into()),
                        must: Some("Regression is covered".into()),
                        evidence: None,
                        severity: None,
                    })]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert!(acceptance_requires_child_report(&acceptance), "premise");
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "just prose, no report block",
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: true,
            })
            .await;
            assert_eq!(ledger.runtime_checks.len(), 1);
            assert_eq!(ledger.runtime_checks[0].id, "attestation");
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Rejected);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn report_optional_still_records_a_check_when_the_report_was_present_but_broken() {
            // The third disjunct of `acceptance.ts:1256`: `parsed.error !== ACCEPTANCE_REPORT_NOT_
            // FOUND`. A MALFORMED report is a real finding even for a caller that would have
            // accepted no report at all.
            let dir = temp_dir();
            let acceptance = attested_policy_requiring_no_report();
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "done\n```acceptance-report\n{ not json at all\n```",
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: true,
            })
            .await;
            assert_ne!(
                ledger.child_report_parse_error.as_deref(),
                Some(ACCEPTANCE_REPORT_NOT_FOUND),
                "premise: this must be a PARSE failure, not an absence"
            );
            assert_eq!(ledger.runtime_checks.len(), 1);
            assert_eq!(ledger.runtime_checks[0].id, "attestation");
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_caller_supplied_report_is_revalidated_not_trusted() {
            // `validateAcceptanceReport(input.report)` (`acceptance.ts:1244-1248` @v0.43.0) — v0.34.0
            // took `input.report` verbatim. The dynamic-group gate is the caller that supplies one.
            let dir = temp_dir();
            let acceptance = attested_policy_requiring_no_report();
            let good = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "",
                cwd: dir.path(),
                report: Some(aggregate_acceptance_report(&[], Some("no children"))),
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(good.status, AcceptanceLedgerStatus::Attested);
            assert!(good.child_report.is_some());

            // A report carrying two criteria that normalize onto the SAME id fails
            // `validateAcceptanceReport` (`acceptance.ts:845,855-858`), so the supplied report is
            // REJECTED rather than recorded — which v0.34.0's verbatim-trust could not do.
            let mut invalid = aggregate_acceptance_report(&[], Some("no children"));
            invalid.criteria_satisfied = Some(vec![
                CriterionReport {
                    id: Some("criterion-1".to_string()),
                    status: CriterionStatus::Satisfied,
                    evidence: "a".to_string(),
                },
                CriterionReport {
                    id: Some("criterion-1".to_string()),
                    status: CriterionStatus::Satisfied,
                    evidence: "b".to_string(),
                },
            ]);
            let bad = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "",
                cwd: dir.path(),
                report: Some(invalid),
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(bad.status, AcceptanceLedgerStatus::Rejected);
            assert!(
                bad.child_report_parse_error
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("Failed to parse acceptance-report: Invalid acceptance-report: "),
                "got {:?}",
                bad.child_report_parse_error
            );
        }

        #[test]
        fn report_optional_suppresses_the_contract_prompt_only_when_no_report_is_required() {
            // `formatAcceptancePrompt`'s own `reportOptional` guard (`acceptance.ts:409`).
            let none_required = attested_policy_requiring_no_report();
            assert_eq!(format_acceptance_prompt(&none_required, true), "");
            assert!(
                !format_acceptance_prompt(&none_required, false).is_empty(),
                "a non-report-optional caller still gets the whole contract block"
            );

            let with_criteria = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    criteria: Some(vec![CriterionInput::Text("Ship the fix".into())]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert!(acceptance_requires_child_report(&with_criteria), "premise");
            assert!(
                !format_acceptance_prompt(&with_criteria, true).is_empty(),
                "a policy that declares criteria is always prompted, reportOptional or not"
            );
        }

        #[test]
        fn duplicate_normalized_criterion_ids_are_rejected() {
            // `criterionIds` (`acceptance.ts:206,218-224` @v0.43.0). The two ids below collapse onto
            // the SAME `normalizedToken`, which is the key `checkCriteriaSatisfied` matches the
            // child's reported criteria against — so the second gate would silently shadow the
            // first and never be enforced.
            let errors = validate_acceptance_input(
                &json!({"criteria": [
                    {"id": "No Staged Files", "must": "a"},
                    {"id": "no_staged__files", "must": "b"}
                ]}),
                "acceptance",
            );
            assert_eq!(
                errors,
                vec![
                    "acceptance.criteria[1].id duplicates normalized criterion id \
                     'no-staged-files'."
                        .to_string()
                ]
            );

            // Distinct ids are fine, and the set is scoped to ONE `criteria[]` array.
            assert!(
                validate_acceptance_input(
                    &json!({"criteria": [{"id": "a", "must": "x"}, {"id": "b", "must": "y"}]}),
                    "acceptance"
                )
                .is_empty()
            );

            // A blank id reports "id is required" ONLY — it must not additionally claim to
            // duplicate the empty token (`acceptance.ts:218-224`'s if/else).
            assert_eq!(
                validate_acceptance_input(
                    &json!({"criteria": [{"id": "  ", "must": "x"}, {"id": "", "must": "y"}]}),
                    "acceptance"
                ),
                vec![
                    "acceptance.criteria[0].id is required.".to_string(),
                    "acceptance.criteria[1].id is required.".to_string(),
                ]
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn checked_mode_rejects_missing_required_evidence() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({"testsAddedOrUpdated": []}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            assert!(acceptance_failure_message(&ledger).unwrap().contains("tests-added evidence missing"));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn checked_mode_rejects_not_satisfied_required_criteria() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    criteria: Some(vec![CriterionInput::Gate(AcceptanceGate {
                        id: Some("regression".into()),
                        must: Some("Regression is covered".into()),
                        evidence: None,
                        severity: None,
                    })]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(
                    json!({"criteriaSatisfied": [{"id": "regression", "status": "not-satisfied", "evidence": "test missing"}]}),
                    "acceptance-report",
                ),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            assert!(acceptance_failure_message(&ledger)
                .unwrap()
                .contains("Required criterion 'regression' was reported as not-satisfied"));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn verified_mode_runs_real_verify_commands() {
            let dir = temp_dir();
            let passing = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Verified),
                    verify: Some(vec![AcceptanceVerifyCommand {
                        id: "pass".into(),
                        command: "exit 0".into(),
                        timeout_ms: Some(10_000),
                        cwd: None,
                        env: None,
                        allow_failure: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let pass_ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &passing,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(pass_ledger.status, AcceptanceLedgerStatus::Verified);
            assert_eq!(pass_ledger.verify_runs.first().map(|r| r.status), Some(VerifyRunStatus::Passed));

            let failing = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Verified),
                    verify: Some(vec![AcceptanceVerifyCommand {
                        id: "fail".into(),
                        command: "exit 7".into(),
                        timeout_ms: Some(10_000),
                        cwd: None,
                        env: None,
                        allow_failure: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let fail_ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &failing,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(fail_ledger.status, AcceptanceLedgerStatus::Rejected);
            // The child's own commandsRun claim of "passed" is IRRELEVANT: the orchestrator observed
            // a real nonzero exit.
            assert_eq!(
                fail_ledger.child_report.as_ref().and_then(|r| r.commands_run.as_ref()).and_then(|c| c.first()).map(|c| c.result.clone()),
                Some(CommandRunResult::Passed)
            );
            assert_eq!(fail_ledger.verify_runs.first().map(|r| r.status), Some(VerifyRunStatus::Failed));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        /// G78 — the review gate hangs off `acceptance.review` (`acceptance.ts:1318-1336`
        /// @v0.43.0), NOT off a `level === "reviewed"` that no longer exists, and it moves ONLY
        /// `status`: `evidence_status` keeps the `checked` the child's evidence actually earned in
        /// all three outcomes. Before the split, "the reviewer has not answered yet" erased that.
        async fn review_gate_records_reviewer_outcomes_without_disturbing_evidence_status() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                        agent: Some("reviewer".into()),
                        focus: None,
                        required: Some(true),
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(acceptance.level, AcceptanceLevel::Checked);

            let reviewed = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: Some(AcceptanceReviewResult {
                    status: ReviewResultStatus::Reviewed,
                    findings: vec![],
                }),
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(reviewed.status, AcceptanceLedgerStatus::Reviewed);
            assert_eq!(reviewed.evidence_status, AcceptanceEvidenceStatus::Checked);
            assert!(acceptance_failure_message(&reviewed).is_none());

            let blockers = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: Some(AcceptanceReviewResult {
                    status: ReviewResultStatus::Blockers,
                    findings: vec![ReviewFinding {
                        severity: ReviewFindingSeverity::Blocker,
                        file: None,
                        issue: "Missing test".into(),
                        rationale: "Acceptance requires test evidence.".into(),
                    }],
                }),
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(blockers.status, AcceptanceLedgerStatus::Rejected);
            assert_eq!(blockers.evidence_status, AcceptanceEvidenceStatus::Checked);
            assert_eq!(
                acceptance_failure_message(&blockers).as_deref(),
                Some("Acceptance review found blockers.")
            );

            let unavailable = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            // `review-required` is NOT `rejected` (`acceptance.ts:1334`): the run is waiting on a
            // reviewer, so it neither passes nor fails the acceptance gate on its own.
            assert_eq!(unavailable.status, AcceptanceLedgerStatus::ReviewRequired);
            assert_eq!(unavailable.evidence_status, AcceptanceEvidenceStatus::Checked);
            assert!(acceptance_failure_message(&unavailable).is_none());
            assert_eq!(
                unavailable.review_result.as_ref().map(|r| r.status),
                Some(ReviewResultStatus::ReviewRequired)
            );
            assert_eq!(
                unavailable
                    .review_result
                    .as_ref()
                    .and_then(|r| r.findings.first())
                    .map(|f| f.issue.as_str()),
                Some("Independent review has not been supplied.")
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        /// G78 — a dynamic-fanout run whose inference recommends review. v0.34.0 escalated the
        /// LEVEL to `reviewed` and then downgraded the gate to `required: false` to avoid turning
        /// an explicit `checked` into a hard blocker; v0.43.0 has neither step, because the level
        /// stops at `checked` and the REQUIRED gate parks the run at `review-required` instead of
        /// rejecting it. `evidence_status` still records the `checked` that was earned.
        async fn dynamic_fanout_review_gate_parks_at_review_required_not_rejected() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement each dynamic item".into()),
                dynamic: true,
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(acceptance.level, AcceptanceLevel::Checked);
            assert!(matches!(
                &acceptance.review,
                Some(ReviewSetting::Gate(g)) if g.required == Some(true)
            ));
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(
                    json!({"criteriaSatisfied": [
                        {"id": "criterion-1", "status": "satisfied", "evidence": "implemented"},
                        {"id": "criterion-2", "status": "satisfied", "evidence": "evidence returned"}
                    ]}),
                    "acceptance-report",
                ),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::ReviewRequired);
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Checked);
            assert!(acceptance_failure_message(&ledger).is_none());
            assert_eq!(
                ledger.review_result.as_ref().map(|r| r.status),
                Some(ReviewResultStatus::ReviewRequired)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        /// G78 — no reviewer result can ever be invented from the child's own evidence. With the
        /// gate explicitly switched OFF (`review: false`, which is falsy at `acceptance.ts:1318`)
        /// the review block does not run at all, so the ledger settles at its evidence level with
        /// NO `reviewResult` — it is never silently promoted to `reviewed`.
        async fn does_not_mark_reviewed_without_an_independent_reviewer_result() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    review: Some(ReviewSetting::Disabled(false)),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(acceptance.level, AcceptanceLevel::Checked);
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Checked);
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Checked);
            assert_eq!(ledger.review_result, None);
            assert!(acceptance_failure_message(&ledger).is_none());
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn zero_child_aggregate_reports_do_not_fabricate_required_evidence() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement dynamic fanout fixes".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: "",
                cwd: dir.path(),
                report: Some(aggregate_acceptance_report(&[], None)),
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            let msg = acceptance_failure_message(&ledger).unwrap();
            assert!(
                ["criterion", "changed-files", "tests-added", "commands-run", "validation-output", "no-staged-files"]
                    .iter()
                    .any(|needle| msg.contains(needle)),
                "unexpected message: {msg}"
            );
        }

        // ---- G78: the checked rung's v0.43.0 rewrite (`acceptance.ts:1268-1278`) ----

        /// G78 — the headline behavioural change of the `evaluateAcceptance` rewrite: a FAILED
        /// structural check on the `checked` rung no longer returns.
        ///
        /// v0.34.0's rung read `if (checks.some(failed)) { status = "rejected"; return ledger; }`,
        /// so a policy whose criteria the child under-reported never ran its `verify[]` commands at
        /// all — the ledger came back with an EMPTY `verifyRuns` and the parent could not tell a
        /// build that was never attempted from one that passed. v0.43.0 declines to PROMOTE on a
        /// failed check and falls through (`acceptance.ts:1274-1277`); the single rejection point is
        /// the combined check at `:1308-1312`, below the verify rung.
        ///
        /// The assertion that pins it is `verify_runs`: a re-introduced early return leaves it
        /// empty. The `runtime_checks` assertions additionally pin the rung's CONTENT and ORDER —
        /// `checkCriteriaSatisfied(...)` first, then `runStructuralChecks(...)`
        /// (`acceptance.ts:1271-1272`) — so dropping or transposing either half is caught too.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_failed_criterion_does_not_short_circuit_the_verify_rung() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Verified),
                    criteria: Some(vec![CriterionInput::Gate(AcceptanceGate {
                        id: Some("regression".into()),
                        must: Some("Regression is covered".into()),
                        evidence: None,
                        severity: None,
                    })]),
                    verify: Some(vec![AcceptanceVerifyCommand {
                        id: "unit".into(),
                        command: "exit 0".into(),
                        timeout_ms: Some(10_000),
                        cwd: None,
                        env: None,
                        allow_failure: None,
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(acceptance.level, AcceptanceLevel::Verified);

            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                // Every declared evidence kind IS present; only the declared CRITERION is reported
                // unsatisfied, so the rung fails on exactly one check.
                output: &report_text(
                    json!({"criteriaSatisfied": [
                        {"id": "regression", "status": "not-satisfied", "evidence": "test missing"}
                    ]}),
                    "acceptance-report",
                ),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;

            // The run IS rejected — but only at `acceptance.ts:1308-1312`, after the verify rung.
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Rejected);
            // THE point of this test: the REAL `verify[]` subprocess still ran and its result is on
            // the ledger. An early return on the failed check leaves this empty.
            assert_eq!(
                ledger.verify_runs.len(),
                1,
                "a failed structural check must not skip the verify[] rung: {ledger:?}"
            );
            assert_eq!(ledger.verify_runs[0].status, VerifyRunStatus::Passed);
            assert_eq!(ledger.verify_runs[0].id, "unit");
            assert_eq!(ledger.verify_runs[0].exit_code, Some(0));

            // The rung's own output: criteria checks FIRST, structural evidence checks after
            // (`acceptance.ts:1271-1272`), all of it on one list.
            let ids: Vec<&str> = ledger
                .runtime_checks
                .iter()
                .map(|c| c.id.as_str())
                .collect();
            assert_eq!(
                ids.first(),
                Some(&"criterion:regression"),
                "checkCriteriaSatisfied's checks come first: {ids:?}"
            );
            assert!(
                ids.iter().skip(1).all(|id| id.starts_with("evidence:") || *id == "no-staged-files"),
                "runStructuralChecks' checks follow them: {ids:?}"
            );
            assert!(
                ids.iter().any(|id| id.starts_with("evidence:")),
                "the structural half must actually have run: {ids:?}"
            );
            assert_eq!(
                ledger
                    .runtime_checks
                    .iter()
                    .filter(|c| c.status == RuntimeCheckStatus::Failed)
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["criterion:regression"],
                "exactly the criterion check failed; everything else passed: {ledger:?}"
            );
            assert!(
                acceptance_failure_message(&ledger)
                    .unwrap()
                    .contains("Required criterion 'regression' was reported as not-satisfied")
            );
        }

        /// G78 — `acceptance.ts:1325`: `else if (acceptance.review.required !== false)`. The gate
        /// parks the ledger at `review-required` unless `required` is EXPLICITLY `false`, so a gate
        /// authored as a bare `{ agent: "reviewer" }` — no `required` key at all — is still a
        /// blocking one. Reading that condition as `required === true` would silently let every
        /// under-specified review gate through as `checked`.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_review_gate_authored_without_required_still_parks_at_review_required() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                        agent: Some("reviewer".into()),
                        focus: None,
                        // `required` DELIBERATELY absent — this is the whole point.
                        required: None,
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert!(
                matches!(&acceptance.review, Some(ReviewSetting::Gate(g)) if g.required.is_none()),
                "premise: the resolved gate carries no `required` at all: {:?}",
                acceptance.review
            );

            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;

            assert_eq!(
                ledger.status,
                AcceptanceLedgerStatus::ReviewRequired,
                "`required` defaults to REQUIRED (`!== false`), not to optional: {ledger:?}"
            );
            // Only `status` moves; the evidence the child actually earned is untouched.
            assert_eq!(ledger.evidence_status, AcceptanceEvidenceStatus::Checked);
            assert_eq!(
                ledger.review_result.as_ref().map(|r| r.status),
                Some(ReviewResultStatus::ReviewRequired)
            );
            assert_eq!(
                ledger
                    .review_result
                    .as_ref()
                    .and_then(|r| r.findings.first())
                    .map(|f| f.issue.as_str()),
                Some("Independent review has not been supplied."),
                "and the synthesized placeholder finding is upstream's (`acceptance.ts:1328-1332`)"
            );
            // `review-required` is not a rejection.
            assert!(acceptance_failure_message(&ledger).is_none());
        }

        /// G78 — `resolveEffectiveAcceptance`'s review resolution, `acceptance.ts:389`:
        /// `explicit.review !== undefined ? explicit.review : inferred.review`. An authored
        /// `review: false` must WIN over an inferred gate.
        ///
        /// The inferred gate here is real: a `dynamic` fanout context takes `inferLevel`'s risky
        /// branch, which returns `review: { agent: "reviewer", required: true }`
        /// (`acceptance.ts:114-120`) — the exact shape that parks a run at `review-required`
        /// forever when nothing supplies a reviewer. Falling back to `inferred.review`
        /// unconditionally would make `review: false` unauthorable on precisely the runs that need
        /// it.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_explicit_review_false_overrides_an_inferred_required_gate() {
            let dir = temp_dir();
            // The same input WITHOUT the override, to prove the inferred gate is genuinely there.
            let inferred_only = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement each dynamic item".into()),
                dynamic: true,
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert!(
                matches!(
                    &inferred_only.review,
                    Some(ReviewSetting::Gate(g)) if g.required == Some(true)
                ),
                "premise: inference produces a REQUIRED review gate here: {:?}",
                inferred_only.review
            );

            let overridden = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement each dynamic item".into()),
                dynamic: true,
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    review: Some(ReviewSetting::Disabled(false)),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(
                overridden.review,
                Some(ReviewSetting::Disabled(false)),
                "an explicit `review: false` replaces the inferred gate outright"
            );
            assert_eq!(
                overridden.level, inferred_only.level,
                "and it changes NOTHING else — the level is still the inferred/explicit max"
            );

            // The behavioural consequence at the gate: `review: false` is falsy at
            // `acceptance.ts:1318`, so the review block never runs.
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &overridden,
                output: &report_text(
                    json!({"criteriaSatisfied": [
                        {"id": "criterion-1", "status": "satisfied", "evidence": "implemented"},
                        {"id": "criterion-2", "status": "satisfied", "evidence": "evidence returned"}
                    ]}),
                    "acceptance-report",
                ),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(
                ledger.status,
                AcceptanceLedgerStatus::Checked,
                "with the gate switched off the run settles at its evidence level: {ledger:?}"
            );
            assert_eq!(ledger.review_result, None);
        }

        // ---- G79: normalizedToken's second pass + the two `skip|skipped` alias groups ----

        /// G79 — `normalizedToken`'s SECOND `/-+/g` pass (`acceptance.ts:514`).
        ///
        /// The first pass (`/[\s_]+/g -> "-"`) only collapses WHITESPACE and UNDERSCORE runs; a
        /// literal `-` the model typed is left alone, so `"Not - Run"` comes out of pass one as
        /// `not---run` and `"NOT__-_SATISFIED"` as `not--satisfied`. Only the second pass folds
        /// those into the canonical single dash. Without it the alias lookup misses and the token
        /// falls through UNCHANGED, which surfaces as a report-validation error rejecting the whole
        /// run over the child's spacing.
        ///
        /// Driven through `parse_acceptance_report`, the live entry point `evaluate_acceptance`
        /// uses — not through the private helper.
        #[test]
        fn a_dashed_and_spaced_status_still_normalizes_onto_the_canonical_token() {
            let parsed = parse_acceptance_report(&report_text(
                json!({
                    "criteriaSatisfied": [
                        {"id": "C -- 1", "status": "NOT__-_SATISFIED", "evidence": "not done"}
                    ],
                    "commandsRun": [
                        {"command": "cargo test", "result": "Not - Run", "summary": "skipped"}
                    ]
                }),
                "acceptance-report",
            ));

            assert_eq!(
                parsed.error, None,
                "a second `-` collapse pass is what makes these tokens recognizable"
            );
            let report = parsed.report.expect("the normalized report parses");
            let criterion = &report.criteria_satisfied.as_ref().unwrap()[0];
            assert_eq!(
                criterion.id.as_deref(),
                Some("c-1"),
                "`C -- 1` -> `c---1` after pass one -> `c-1` after pass two"
            );
            assert_eq!(criterion.status, CriterionStatus::NotSatisfied);
            assert_eq!(
                report.commands_run.as_ref().unwrap()[0].result,
                CommandRunResult::NotRun,
                "`Not - Run` -> `not---run` after pass one -> `not-run` after pass two"
            );
        }

        /// G79 — the two alias groups the existing normalization test never touches:
        /// `normalizeCommandResult`'s `not-run|not-executed|skip|skipped` (`acceptance.ts:531`) and
        /// `normalizeCriterionStatus`'s `not-applicable|n-a|na|skip|skipped` (`:522`).
        ///
        /// `skip`/`skipped` appear in BOTH tables and mean different canonical values depending on
        /// which field they land in — the one place these tables are easy to get wrong.
        #[test]
        fn the_not_run_and_not_applicable_alias_groups_fold_onto_their_canonical_values() {
            for alias in ["not-run", "not_executed", "NOT EXECUTED", "skip", "Skipped"] {
                let parsed = parse_acceptance_report(&report_text(
                    json!({"commandsRun": [
                        {"command": "cargo test", "result": alias, "summary": "not attempted"}
                    ]}),
                    "acceptance-report",
                ));
                assert_eq!(parsed.error, None, "`{alias}` must be a recognized commandsRun result");
                let report = parsed.report.expect("the report parses");
                assert_eq!(
                    report.commands_run.as_ref().unwrap()[0].result,
                    CommandRunResult::NotRun,
                    "`{alias}` is an alias of `not-run` (acceptance.ts:531)"
                );
            }

            // NB `n-a`, not `n/a`: `normalizedToken` collapses whitespace and `_`, never `/`, so a
            // literal slash is NOT an alias upstream either (`acceptance.ts:513-514,522`).
            for alias in ["not-applicable", "N A", "n_a", "na", "skip", "Skipped"] {
                let parsed = parse_acceptance_report(&report_text(
                    json!({"criteriaSatisfied": [
                        {"id": "c1", "status": alias, "evidence": "does not apply here"}
                    ]}),
                    "acceptance-report",
                ));
                assert_eq!(parsed.error, None, "`{alias}` must be a recognized criterion status");
                let report = parsed.report.expect("the report parses");
                assert_eq!(
                    report.criteria_satisfied.as_ref().unwrap()[0].status,
                    CriterionStatus::NotApplicable,
                    "`{alias}` is an alias of `not-applicable` (acceptance.ts:522)"
                );
            }

            // A token that is in NEITHER table is still returned unchanged and then rejected by the
            // enum check, so the alias tables cannot be widened by accident.
            let unknown = parse_acceptance_report(&report_text(
                json!({"commandsRun": [
                    {"command": "cargo test", "result": "deferred", "summary": "later"}
                ]}),
                "acceptance-report",
            ));
            assert!(
                unknown
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("commandsRun[0].result: expected one of \"passed\", \"failed\", \"not-run\"; got \"deferred\""),
                "{unknown:?}"
            );
        }

        /// G79 — `validateStringArrayField`'s v0.43.0 tightening (`acceptance.ts:827` added
        /// `|| !item.trim()`): a BLANK entry is no longer admissible evidence.
        ///
        /// This asserts the RULE, not the message: a report whose `changedFiles` is `["   "]` must
        /// not parse at all, so a `checked` policy that requires `changed-files` evidence REJECTS
        /// the run instead of accepting a whitespace string as proof a file changed. The pre-v0.43.0
        /// check (`typeof item !== "string"`) admitted it.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_blank_evidence_entry_is_not_admissible_evidence() {
            // The parser refuses the report outright.
            let blank = parse_acceptance_report(&report_text(
                json!({"changedFiles": ["   ", "\t\n"]}),
                "acceptance-report",
            ));
            assert!(
                blank.report.is_none(),
                "a blank array entry must invalidate the whole report: {blank:?}"
            );
            // Both entries are flagged, not just the first — the check is per-item.
            let err = blank.error.as_deref().unwrap();
            assert!(err.contains("changedFiles[0]"), "{err}");
            assert!(err.contains("changedFiles[1]"), "{err}");

            // ... and the same array with ONE real entry parses, so the rule is "blank", not
            // "array".
            let real = parse_acceptance_report(&report_text(
                json!({"changedFiles": ["src/file.rs"]}),
                "acceptance-report",
            ));
            assert_eq!(real.error, None, "{real:?}");

            // The consequence at the live gate: a policy gated on `changed-files` evidence rejects
            // the blank report rather than counting it.
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Checked),
                    evidence: Some(vec![AcceptanceEvidenceKind::ChangedFiles]),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({"changedFiles": ["   "]}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                file_output: None,
                review_result: None,
                memo: None,
                report_optional: false,
            })
            .await;
            assert_eq!(
                ledger.status,
                AcceptanceLedgerStatus::Rejected,
                "a whitespace-only changedFiles entry is not evidence: {ledger:?}"
            );
            assert!(
                ledger
                    .child_report_parse_error
                    .as_deref()
                    .unwrap()
                    .contains("changedFiles[0]: expected non-empty string"),
                "{ledger:?}"
            );
        }

        // ---- validateAcceptanceInput ----

        #[test]
        fn validates_invalid_disable_and_verify_shapes() {
            let v = |value: Value| validate_acceptance_input(&value, "acceptance");
            assert_eq!(v(json!({"level": "none"})), vec!["acceptance.reason is required when level is none.".to_string()]);
            assert_eq!(v(json!({"verify": [{"id": "missing-command"}]})), vec!["acceptance.verify[0].command is required.".to_string()]);
            assert_eq!(
                v(json!({"verify": [{"id": "fractional", "command": "npm test", "timeoutMs": 1.5}]})),
                vec!["acceptance.verify[0].timeoutMs must be an integer >= 1.".to_string()]
            );
            assert!(v(json!(false)).is_empty());
            assert!(v(json!("checked")).is_empty());
            assert!(v(json!({"criteria": ["ship the fix"], "review": false, "stopRules": ["stay scoped"]})).is_empty());
            assert!(v(json!({"criteria": [{"id": "missing-must"}]})).iter().any(|e| e.contains("acceptance.criteria[0].must is required")));
            assert!(v(json!({"criteria": [123]})).iter().any(|e| e.contains("acceptance.criteria[0] must be a string or an object")));
            // v0.43.0 routes this through `unsupportedEvidenceKindMessage` (`acceptance.ts:171-174`),
            // which ECHOES the offending value and appends `ACCEPTANCE_EVIDENCE_HELP` (`:48`).
            // Asserted whole rather than by substring so the guidance cannot silently regress.
            assert_eq!(
                v(json!({"evidence": ["bogus"]})),
                vec![
                    "acceptance.evidence[0] \"bogus\" is not a supported evidence kind. Supported \
                     evidence kinds: changed-files, tests-added, commands-run, validation-output, \
                     residual-risks, no-staged-files, diff-summary, review-findings, manual-notes. \
                     Example: { level: \"checked\", evidence: [\"commands-run\", \"changed-files\"] }."
                        .to_string()
                ]
            );
            // A NON-string entry gets the same message with no echo (`typeof item === "string" ?
            // ` "${item}"` : ""`, `acceptance.ts:172`).
            assert!(
                v(json!({"evidence": [7]}))
                    .iter()
                    .any(|e| e.starts_with("acceptance.evidence[0] is not a supported evidence kind. Supported evidence kinds: "))
            );
            assert!(v(json!({"review": true})).iter().any(|e| e.contains("acceptance.review must be false or an object")));
            assert!(v(json!({"review": {"required": "yes"}})).iter().any(|e| e.contains("acceptance.review.required must be a boolean")));
            assert!(v(json!({"stopRules": [123]})).iter().any(|e| e.contains("acceptance.stopRules[0] must be a string")));
            assert!(v(json!({"surprise": true})).iter().any(|e| e.contains("acceptance.surprise is not supported")));
        }
    }
}

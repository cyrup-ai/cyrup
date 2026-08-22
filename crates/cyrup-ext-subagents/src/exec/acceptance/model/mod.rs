//! The C12 port of pi's acceptance criteria/evidence model — the survivor of the two-API
//! collapse the banner below records. Owns the criteria/evidence/report shapes, level
//! inference, report parsing and validation, the runtime checks, and the REAL `verify[]`
//! runner both gates call.

// ================================================================================================
// C12: faithful port of the pi acceptance criteria/evidence model
// (`pi-subagents/src/runs/shared/acceptance.ts` + `src/shared/types.ts:639-802`).
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
//     `crate::exec::acceptance::AcceptanceLedger::verify_results` holds THIS type.
//   * `validateAcceptanceInput` — one implementation, here; `discovery/chains.rs` delegates to it
//     rather than carrying the second transcription it used to.
//   * `evidenceStatus` — one meaning, here (`AcceptanceEvidenceStatus`); the enum-lattice ledger
//     carries the same type, projected via `crate::exec::acceptance::AcceptanceStatus::evidence_status`.
//
// What has NOT collapsed: the LEDGER struct and the verdict that fills it. `crate::exec::acceptance::AcceptanceLedger`
// still computes `status`/`detail` from `crate::exec::acceptance::AcceptanceContract`'s required-level lattice rather
// than from `AcceptanceLevel` + `runtimeChecks`, and it does not carry upstream's `runtimeChecks`,
// `childReport`, `childReportParseError`, `reviewResult`, `explicit`, `inferredReason` or
// `criteria`. Finishing the collapse means `exec/mod.rs::run_sync` calling `evaluate_acceptance`
// here and `SingleResult.acceptance` becoming `AcceptanceLedger` here.
// ================================================================================================

#[cfg(test)]
pub(crate) mod testsupport;

pub mod aggregate;
pub mod checks;
pub mod evaluate;
pub mod level;
pub mod prompt;
pub mod report;
pub mod types;
pub mod validate_input;
pub mod verify;

// Every name below kept its pre-split path `crate::exec::acceptance::model::<name>`;
// these re-exports are what hold that surface across the split.
pub use aggregate::{
    aggregate_acceptance_report, AggregateChild,
};
pub use checks::{
    check_criteria_satisfied, run_structural_checks,
};
pub use evaluate::{
    acceptance_failure_message, evaluate_acceptance, EvaluateAcceptanceInput,
};
pub use level::{
    normalize_acceptance_input, normalize_criteria, resolve_effective_acceptance, unique_evidence, AcceptanceResolveInput, SubagentRunMode,
};
pub use prompt::{
    acceptance_requires_child_report, format_acceptance_prompt,
};
pub use report::parse::{
    parse_acceptance_report, parse_acceptance_report_sources, strip_acceptance_report, strip_acceptance_report_from_message_text, ACCEPTANCE_REPORT_NOT_FOUND, ParsedAcceptanceReport,
};
pub use types::{
    AcceptanceConfig, AcceptanceEvidenceKind, AcceptanceEvidenceStatus, AcceptanceGate, AcceptanceInput, AcceptanceLedger, AcceptanceLedgerStatus, AcceptanceLevel, AcceptanceReport, AcceptanceReviewGate, AcceptanceReviewResult, AcceptanceRuntimeCheck, AcceptanceVerifyCommand, AcceptanceVerifyResult, CommandRunReport, CommandRunResult, CriterionInput, CriterionReport, CriterionStatus, GateSeverity, ResolvedAcceptanceConfig, ResolvedAcceptanceGate, ReviewFinding, ReviewFindingSeverity, ReviewResultStatus, ReviewSetting, RuntimeCheckStatus, SerializableGate, VerifyRunStatus, VerifyWorkspaceKind, VerifyWorkspaceState,
};
pub use validate_input::{
    validate_acceptance_input, EXPLICIT_REVIEWED_UNAVAILABLE,
};
pub use verify::memo::{
    read_verify_workspace_state, run_memoized_verify_command, run_memoized_verify_command_with_cancel, VerifyMemoContext,
};
pub use verify::redact::redact_verify_env;
pub use verify::run::{
    run_verify_command, run_verify_command_with_cancel, DEFAULT_VERIFY_TIMEOUT_MS, VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE, VERIFY_TIMED_OUT_MESSAGE,
};

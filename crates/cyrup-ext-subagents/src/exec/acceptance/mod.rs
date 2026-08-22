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

pub mod lattice;
pub mod model;

// The lattice half's public surface, re-exported at the paths it had before the split:
// `crate::exec::acceptance::<name>`. `model` keeps its own paths through its own re-exports.
pub use lattice::{
    build_timed_out_acceptance_ledger, AcceptanceLedger, AcceptanceStatus,
};
pub use lattice::contract::{
    AcceptanceContract, ReviewerResult, VerifyCommand,
};
pub use lattice::gate::{
    evaluate_acceptance, evaluate_acceptance_with_cancel, CleanCompletionGate,
};
pub use lattice::inject::inject_acceptance_contract;
pub use lattice::lowering::lower_acceptance_input;
pub use lattice::post_hoc::{
    apply_post_hoc_correction, ACCEPTANCE_REJECTED_EXIT_CODE, PostHocCorrection,
};
pub use lattice::report_source::AcceptanceFileOutput;
pub use lattice::verify::{
    run_verify_commands, run_verify_commands_memoized, run_verify_commands_memoized_with_cancel,
};

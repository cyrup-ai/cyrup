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
use std::time::Duration;

use crate::exec::completion_guard::CompletionMutationGuardResult;
use crate::exec::output::looks_like_acceptance_report;

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
}

/// The full acceptance-provenance record attached to one run's result (func-SA §4.3's
/// `AcceptanceLedger`, arch-SA §3.4).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptanceLedger {
    /// The achieved status.
    pub status: AcceptanceStatus,
    /// A human-readable detail string: which check ran, which `verify[]` command failed and why,
    /// or `None` when `status` speaks for itself (e.g. `NotRequired`).
    pub detail: Option<String>,
    /// The individual `verify[]` command outcomes that contributed to `status`, in declaration
    /// order — present whenever [`run_verify_commands`] actually ran (i.e. the contract's
    /// required level was `Verified` or above), empty otherwise. Kept on the ledger itself (not
    /// merely folded into `detail`'s free text) so a caller/UI can render per-command
    /// pass/fail without re-parsing `detail`.
    #[serde(default)]
    pub verify_results: Vec<VerifyCommandResult>,
}

impl AcceptanceLedger {
    /// Construct a `NotRequired` ledger with no detail — the default outcome when no contract
    /// resolves for a task at all.
    #[must_use]
    pub fn not_required() -> Self {
        Self {
            status: AcceptanceStatus::NotRequired,
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
            detail: Some("Acceptance was not evaluated because the subagent timed out.".to_string()),
            verify_results: Vec::new(),
        }
    }
}

// ============================================================================================
// R-SA-023: Acceptance contract injection
// ============================================================================================

/// One declared verification command (func-SA `acceptance.verify[]`): a literal shell command
/// string, executed via a real subprocess (R-SA-032, [`run_verify_commands`]) — never parsed or
/// interpreted beyond being handed to a shell, and never treated as satisfied by anything the
/// child itself claims about it.
pub type VerifyCommand = String;

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
}

impl AcceptanceContract {
    /// Build an explicit, caller-supplied contract (the "explicit `acceptance` param" half of
    /// R-SA-023's resolution rule). `required_level` MUST NOT be [`AcceptanceStatus::Rejected`] —
    /// callers passing that are almost certainly confusing an outcome with a requirement;
    /// this constructor clamps it down to [`AcceptanceStatus::Reviewed`] (the highest real,
    /// requestable level) defensively rather than accepting a nonsensical contract, since this
    /// crate's no-panic policy forbids failing loudly here and a silently-dropped contract would
    /// be worse than a clamped one.
    #[must_use]
    pub fn explicit(required_level: AcceptanceStatus, verify: Vec<VerifyCommand>) -> Self {
        Self {
            required_level: clamp_requestable_level(required_level),
            verify,
            explicit: true,
            reviewer_result: None,
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
        self
    }

    /// Build the heuristically-inferred default contract (the "heuristic inference from agent
    /// name/task text" half of R-SA-023's resolution rule), used only when the caller supplied no
    /// explicit `acceptance` param at all.
    ///
    /// The heuristic itself: an implementation-expecting task (per
    /// [`crate::exec::completion_guard::expects_implementation_mutation`] — this module
    /// deliberately reuses that already-built, already-tested classifier rather than inventing a
    /// second, competing "does this task look like it needs verification" heuristic; the two
    /// questions ("does this need a mutation" and "does this need acceptance evidence") are
    /// answered by materially the same signal) defaults to requiring [`AcceptanceStatus::Checked`]
    /// with no `verify[]` commands (nothing was explicitly declared to run); a task that does not
    /// read as implementation-expecting defaults to [`AcceptanceStatus::NotRequired`] — there is
    /// nothing to gate on a pure research/review task by default.
    #[must_use]
    pub fn heuristic_default(agent_local_name: &str, task: &str) -> Self {
        let expects_mutation = crate::exec::completion_guard::expects_implementation_mutation(
            agent_local_name,
            task,
        );
        let required_level = if expects_mutation {
            AcceptanceStatus::Checked
        } else {
            AcceptanceStatus::NotRequired
        };
        Self {
            required_level,
            verify: Vec::new(),
            explicit: false,
            reviewer_result: None,
        }
    }

    /// Whether this contract requires no gate evaluation at all — `required_level ==
    /// NotRequired` and no reviewer result is attached. [`evaluate_acceptance`] short-circuits on
    /// this so a task with no acceptance requirement never even inspects the transcript.
    #[must_use]
    pub fn is_no_op(&self) -> bool {
        self.required_level == AcceptanceStatus::NotRequired && self.reviewer_result.is_none()
    }
}

/// Defensive clamp for [`AcceptanceContract::explicit`]'s `required_level` — see that
/// constructor's doc comment.
fn clamp_requestable_level(level: AcceptanceStatus) -> AcceptanceStatus {
    match level {
        AcceptanceStatus::Rejected => AcceptanceStatus::Reviewed,
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

/// The exact heading this module injects and later scans for — kept as a named constant so
/// [`inject_acceptance_contract`] and any future re-detection logic never drift out of sync with
/// each other over a hand-typed literal.
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
/// The injected block instructs the child to end its answer with a fenced ` ```acceptance-report
/// ` JSON block carrying, at minimum, `criteriaSatisfied: bool` plus whichever of the
/// `exec/output.rs::ACCEPTANCE_REPORT_COMPANION_KEYS` fields are relevant — this wording is
/// deliberately compatible with [`crate::exec::output::looks_like_acceptance_report`]'s own
/// detection rules so a compliant child's block is reliably picked up by both R-SA-029's
/// final-output extraction AND this module's own [`extract_acceptance_report`].
#[must_use]
pub fn inject_acceptance_contract(task: &str, contract: &AcceptanceContract) -> String {
    if contract.is_no_op() {
        return task.to_string();
    }

    let mut block = String::new();
    block.push_str(ACCEPTANCE_CONTRACT_HEADING);
    block.push('\n');
    block.push_str(&format!(
        "Required acceptance level for this task: **{}**.\n",
        contract.required_level.as_wire_str()
    ));
    block.push_str(
        "Before finishing, you MUST end your final answer with a fenced code block, tagged \
         `acceptance-report`, containing a single JSON object with at minimum the key \
         `criteriaSatisfied` (boolean) describing whether you believe the task's acceptance \
         criteria were met. Include any of the following keys that are relevant to this task: \
         `changedFiles`, `testsAddedOrUpdated`, `commandsRun`, `validationOutput`, \
         `residualRisks`, `noStagedFiles`, `diffSummary`, `reviewFindings`, `manualNotes`.\n",
    );
    if !contract.verify.is_empty() {
        block.push_str(
            "Note: the orchestrator will independently execute the following verification \
             command(s) after you finish and will NOT rely on your own report of their outcome:\n",
        );
        for cmd in &contract.verify {
            block.push_str(&format!("- `{cmd}`\n"));
        }
    }

    if task.is_empty() {
        block
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

/// The default bound on how long a single `verify[]` command may run before it is treated as a
/// failure (this crate introduces no configuration surface for this in v1; a generous fixed bound
/// keeps a hung verification command from blocking the acceptance gate — and therefore the whole
/// run — indefinitely, mirroring `spawn::worktree::DEFAULT_HOOK_TIMEOUT`'s identical rationale for
/// its own bounded external-command call).
pub const DEFAULT_VERIFY_TIMEOUT: Duration = Duration::from_millis(300_000);

/// The observed outcome of ACTUALLY EXECUTING one `verify[]` command as a real OS subprocess
/// (R-SA-032, DI-SA-5) — never a child's self-report.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifyCommandResult {
    /// The literal command string that was executed.
    pub command: String,
    /// The real observed exit code, or `None` if the process was terminated by a signal (Unix)
    /// rather than exiting normally.
    pub exit_code: Option<i32>,
    /// Whether this command counts as passed: `exit_code == Some(0)`.
    pub passed: bool,
    /// Combined stdout+stderr tail (bounded, see [`run_one_verify_command`]'s doc comment) —
    /// kept for the rejection detail text so a caller/UI can show WHY a `verify[]` command
    /// failed, not merely that it did.
    pub output_tail: String,
    /// Set when the command could not even be spawned (binary not found, permission denied) or
    /// timed out — distinct from a genuine nonzero exit, since both cases still make `passed`
    /// `false` but a UI/log message should describe them differently.
    pub spawn_error: Option<String>,
}

/// The largest number of trailing bytes of a `verify[]` command's combined output retained in
/// [`VerifyCommandResult::output_tail`] — bounded so a pathologically verbose verification command
/// (e.g. a build tool dumping megabytes of warnings) cannot make the acceptance ledger itself
/// unbounded, mirroring `exec/output.rs::OutputCap`'s identical bounding rationale applied here to
/// a narrower, fixed budget appropriate for a "why did this fail" snippet rather than a full
/// delivered result.
const VERIFY_OUTPUT_TAIL_BYTES: usize = 4096;

/// R-SA-032 / DI-SA-5 (MUST) — actually execute every command in `commands`, IN ORDER, as a real
/// OS subprocess each, observing each one's real exit code. Returns one [`VerifyCommandResult`]
/// per command, always in the same order as `commands` — this function does NOT short-circuit on
/// the first failure (a caller wants to see every command's real outcome for a rejected run's
/// detail text, not just the first one that failed), but callers deciding overall pass/fail MUST
/// require every result's `passed` to be `true` (see [`evaluate_acceptance`]).
///
/// Each command is executed via the platform shell (`/bin/sh -c <command>` on Unix,
/// `cmd /C <command>` on Windows) — a `verify[]` entry is, by func-SA's own data model, a literal
/// shell command string (e.g. `"cargo test --workspace"`), not a pre-split argv, so shell
/// interpretation is required to honor the declared string faithfully (pipes, `&&` chains, globs,
/// etc.), exactly mirroring how `spawn::worktree`'s own hook contract and `cyrup-tools::ops::local`
/// treat a caller-declared shell command string (this module does not depend on the latter crate,
/// per this file's module doc, but the shelling-out *shape* is the same well-established pattern).
///
/// This function never panics and never returns an `Err` for an individual command's failure —
/// a command that fails to spawn, exits nonzero, or times out all surface as
/// `VerifyCommandResult { passed: false, .. }` with an explanatory `spawn_error`/`output_tail`,
/// never a propagated error that would abort evaluation of the REMAINING commands.
pub async fn run_verify_commands(
    commands: &[VerifyCommand],
    cwd: &Path,
) -> Vec<VerifyCommandResult> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        results.push(run_one_verify_command(command, cwd, DEFAULT_VERIFY_TIMEOUT).await);
    }
    results
}

/// The single-command core [`run_verify_commands`] loops over, factored out so tests can inject a
/// shorter timeout without waiting out [`DEFAULT_VERIFY_TIMEOUT`].
///
/// On expiry the command is KILLED, never abandoned: `crate::spawn::signal::terminate_on_timeout`
/// sends `SIGTERM` and then a hard `SIGKILL` a second later, targeting the command's own process
/// group. This ports upstream `runVerifyCommand`'s `abortVerification`
/// (`pi-subagents/src/runs/shared/acceptance.ts:742-758` @v0.34.0), which does exactly that —
/// `child.kill("SIGTERM")` plus a 1000 ms `setTimeout` hard `child.kill("SIGKILL")` — and this
/// function returns only once the OS process is confirmed reaped.
async fn run_one_verify_command(
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> VerifyCommandResult {
    let mut cmd = shell_command(command);
    cmd.current_dir(cwd);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        // Own process group, exactly mirroring `spawn::SpawnedChild::spawn`'s rationale: a
        // verify[] command is real work the orchestrator itself launched and is responsible for
        // reaping/timing out cleanly, independent of the parent's own signal disposition.
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return VerifyCommandResult {
                command: command.to_string(),
                exit_code: None,
                passed: false,
                output_tail: String::new(),
                spawn_error: Some(format!("failed to spawn verify command: {err}")),
            };
        }
    };

    // Drain both pipes on their own tasks rather than via `Child::wait_with_output`. That
    // convenience method takes `self` BY VALUE, so racing it against `tokio::time::timeout`
    // consumed the only handle to the process: the elapsed arm dropped the future, `kill_on_drop`
    // was never set, and a hung `cargo test`/`npm run e2e` survived its own timeout for the
    // machine's uptime — inside a process group `process_group(0)` above has deliberately detached
    // from the terminal, so the user's Ctrl-C could not reach it either. Keeping the `Child`
    // binding is what makes the timeout arm able to actually kill (upstream `abortVerification`,
    // acceptance.ts:742-758 @v0.34.0).
    let stdout_task = child.stdout.take().map(spawn_pipe_drain);
    let stderr_task = child.stderr.take().map(spawn_pipe_drain);

    let waited = tokio::select! {
        biased;
        result = child.wait() => Some(result),
        () = tokio::time::sleep(timeout) => None,
    };

    let Some(waited) = waited else {
        // Timed out: SIGTERM, then a hard SIGKILL 1s later, targeting the child's process GROUP
        // (it leads one — see `process_group(0)` above), so the descendants the command spawned
        // die with it. `terminate_on_timeout` returns only once the process is CONFIRMED reaped.
        let _ = crate::spawn::signal::terminate_on_timeout(&mut child).await;
        // The drains would normally end on their own now that the writers are dead, but a pipe
        // could still be held by something that escaped the group; abort rather than risk hanging
        // acceptance evaluation. The tail was already empty on this path before the fix.
        if let Some(task) = stdout_task {
            task.abort();
        }
        if let Some(task) = stderr_task {
            task.abort();
        }
        return VerifyCommandResult {
            command: command.to_string(),
            exit_code: None,
            passed: false,
            output_tail: String::new(),
            spawn_error: Some(format!(
                "verify command exceeded its {}ms timeout and was terminated",
                timeout.as_millis()
            )),
        };
    };

    let mut combined = drained(stdout_task).await;
    combined.extend_from_slice(&drained(stderr_task).await);

    match waited {
        Ok(status) => {
            let exit_code = status.code();
            let passed = exit_code == Some(0);
            VerifyCommandResult {
                command: command.to_string(),
                exit_code,
                passed,
                output_tail: tail_utf8_lossy(&combined, VERIFY_OUTPUT_TAIL_BYTES),
                spawn_error: None,
            }
        }
        Err(err) => VerifyCommandResult {
            command: command.to_string(),
            exit_code: None,
            passed: false,
            output_tail: String::new(),
            spawn_error: Some(format!("failed to wait on verify command: {err}")),
        },
    }
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

/// The last (up to) `max_bytes` bytes of `data`, decoded UTF-8-lossily — used only for a bounded
/// "why did this fail" tail, so lossy decoding (rather than [`crate::exec::output`]'s stricter
/// UTF-8-boundary-safe truncation) is an acceptable, deliberate simplification here: this text is
/// diagnostic detail, never re-delivered as the run's own primary output.
fn tail_utf8_lossy(data: &[u8], max_bytes: usize) -> String {
    let start = data.len().saturating_sub(max_bytes);
    let tail = data.get(start..).unwrap_or(data);
    String::from_utf8_lossy(tail).into_owned()
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
///    against every declared command; achieving [`AcceptanceStatus::Verified`] requires **every**
///    result's `passed` to be `true` — a single failing (or unspawnable/timed-out) command caps
///    the achieved level below `Verified` regardless of how many others passed, and regardless of
///    anything the child's own report claims.
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
#[must_use]
pub async fn evaluate_acceptance(
    contract: &AcceptanceContract,
    gate: CleanCompletionGate,
    final_output: Option<&str>,
    completion_guard: CompletionMutationGuardResult,
    verify_cwd: &Path,
) -> AcceptanceLedger {
    if !gate.is_clean() {
        return AcceptanceLedger::not_required();
    }
    if contract.is_no_op() {
        return AcceptanceLedger::not_required();
    }

    // Step 2: self-report floor (Claimed / Attested).
    let mut achieved = self_report_floor(final_output);

    // Step 3: Checked — orchestrator-observed structural evidence.
    let mut detail: Vec<String> = Vec::new();
    if contract.required_level >= AcceptanceStatus::Checked {
        if completion_guard.triggered {
            detail.push(
                "checked: completion-mutation guard triggered (implementation was expected but \
                 no mutating tool call was observed)"
                    .to_string(),
            );
        } else {
            achieved = achieved.max(AcceptanceStatus::Checked);
        }
    }

    // Step 4: Verified — REAL verify[] execution (DI-SA-5's whole reason for being).
    let mut verify_results = Vec::new();
    if contract.required_level >= AcceptanceStatus::Verified {
        if contract.verify.is_empty() {
            detail.push("verified: no verify[] commands were declared".to_string());
        } else {
            verify_results = run_verify_commands(&contract.verify, verify_cwd).await;
            let all_passed = verify_results.iter().all(|r| r.passed);
            if all_passed {
                achieved = achieved.max(AcceptanceStatus::Verified);
            } else {
                let failed: Vec<&str> = verify_results
                    .iter()
                    .filter(|r| !r.passed)
                    .map(|r| r.command.as_str())
                    .collect();
                detail.push(format!(
                    "verified: {} of {} verify[] command(s) failed: {}",
                    failed.len(),
                    verify_results.len(),
                    failed.join(", ")
                ));
            }
        }
    }

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
            detail: Some(reason),
            verify_results,
        }
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
    let has_companion_evidence = crate::exec::output::ACCEPTANCE_REPORT_COMPANION_KEYS
        .iter()
        .any(|key| text.contains(&format!("\"{key}\"")));
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

    #[test]
    fn heuristic_default_is_not_required_for_review_only_tasks() {
        let contract =
            AcceptanceContract::heuristic_default("worker", "Review only: return findings");
        assert_eq!(contract.required_level, AcceptanceStatus::NotRequired);
        assert!(contract.is_no_op());
    }

    #[test]
    fn explicit_contract_is_marked_explicit_and_carries_verify_commands() {
        let contract = AcceptanceContract::explicit(
            AcceptanceStatus::Verified,
            vec!["true".to_string()],
        );
        assert!(contract.explicit);
        assert_eq!(contract.required_level, AcceptanceStatus::Verified);
        assert_eq!(contract.verify, vec!["true".to_string()]);
    }

    #[test]
    fn explicit_contract_clamps_a_nonsensical_rejected_requested_level() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Rejected, vec![]);
        assert_eq!(contract.required_level, AcceptanceStatus::Reviewed);
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

    #[test]
    fn no_op_contract_leaves_task_text_unchanged() {
        let contract = AcceptanceContract::heuristic_default("researcher", "Investigate the bug");
        let out = inject_acceptance_contract("Investigate the bug", &contract);
        assert_eq!(out, "Investigate the bug");
    }

    #[test]
    fn required_contract_appends_a_machine_parseable_acceptance_contract_block() {
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Verified, vec!["cargo test".to_string()]);
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

    // ---------------------------------------------------------------------------------------
    // run_verify_commands / run_one_verify_command: REAL subprocess execution (no mocks)
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_command_that_exits_zero_is_recorded_as_passed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command("exit 0", dir.path(), Duration::from_secs(5)).await;
        assert!(result.passed);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.spawn_error.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_command_that_exits_nonzero_is_recorded_as_failed_with_real_exit_code() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command("exit 7", dir.path(), Duration::from_secs(5)).await;
        assert!(!result.passed);
        assert_eq!(result.exit_code, Some(7));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_tail_captures_real_combined_stdout_and_stderr() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command(
            "echo out-marker; echo err-marker 1>&2; exit 1",
            dir.path(),
            Duration::from_secs(5),
        )
        .await;
        assert!(!result.passed);
        assert!(result.output_tail.contains("out-marker"));
        assert!(result.output_tail.contains("err-marker"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn command_runs_in_the_declared_working_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("marker.txt"), "hi").expect("seed file");
        let result =
            run_one_verify_command("test -f marker.txt", dir.path(), Duration::from_secs(5)).await;
        assert!(result.passed, "the file must be visible relative to cwd");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hanging_command_times_out_and_is_recorded_as_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = run_one_verify_command(
            "sleep 5",
            dir.path(),
            Duration::from_millis(100),
        )
        .await;
        assert!(!result.passed);
        assert!(result.spawn_error.as_deref().unwrap_or_default().contains("timeout"));
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
            "echo $$ > pid; exec sleep 300",
            dir.path(),
            Duration::from_millis(200),
        )
        .await;

        assert!(!result.passed);
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
            "sleep 300 & echo $! > descendant; wait",
            dir.path(),
            Duration::from_millis(200),
        )
        .await;

        assert!(!result.passed);
        let pid = wait_for_published_pid(&descendant_pid_file, Duration::from_secs(5)).await;
        assert!(
            wait_for_pid_gone(pid, Duration::from_secs(5)).await,
            "the descendant pid {pid} the verify command spawned must die with it — the kill \
             targets the command's process GROUP, not just its direct pid"
        );
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
            "trap '' TERM; echo $$ > pid; while true; do sleep 1; done",
            dir.path(),
            Duration::from_millis(200),
        )
        .await;
        let elapsed = started.elapsed();

        assert!(!result.passed);
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
            "exit 1".to_string(), // fails
            "exit 0".to_string(), // still runs, passes
        ];
        let results = run_verify_commands(&commands, dir.path()).await;
        assert_eq!(results.len(), 2, "both commands must run even though the first failed");
        assert!(!results[0].passed);
        assert!(results[1].passed);
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
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec!["exit 0".to_string()]);

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some("I fixed the bug.\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```"),
            no_guard_trigger(),
            dir.path(),
        )
        .await;

        assert_eq!(
            ledger.status,
            AcceptanceStatus::Verified,
            "a REAL verify[] command that actually exits 0 must reach Verified, got: {ledger:?}"
        );
        assert_eq!(ledger.verify_results.len(), 1);
        assert!(ledger.verify_results[0].passed);
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
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec!["exit 1".to_string()]);

        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            Some(
                "Everything passed!\n```acceptance-report\n{\"criteriaSatisfied\": true}\n```",
            ),
            no_guard_trigger(),
            dir.path(),
        )
        .await;

        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert_eq!(ledger.verify_results.len(), 1);
        assert!(!ledger.verify_results[0].passed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_failing_command_among_several_caps_below_verified_even_if_others_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(
            AcceptanceStatus::Verified,
            vec!["exit 0".to_string(), "exit 1".to_string(), "exit 0".to_string()],
        );
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path())
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert_eq!(ledger.verify_results.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn not_clean_gate_short_circuits_to_not_required_regardless_of_contract() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract =
            AcceptanceContract::explicit(AcceptanceStatus::Verified, vec!["exit 0".to_string()]);
        let dirty_gate = CleanCompletionGate {
            exit_code: 1,
            detached: false,
            interrupted: false,
            timed_out: false,
        };
        let ledger =
            evaluate_acceptance(&contract, dirty_gate, None, no_guard_trigger(), dir.path()).await;
        assert_eq!(ledger.status, AcceptanceStatus::NotRequired);
        assert!(ledger.verify_results.is_empty(), "must not even run verify[] on a non-clean gate");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checked_level_is_satisfied_by_a_non_triggered_completion_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path())
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
            evaluate_acceptance(&contract, clean_gate(), None, triggered, dir.path()).await;
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
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path())
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Reviewed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reviewed_level_rejected_when_reviewer_result_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);
        contract.required_level = AcceptanceStatus::Reviewed; // demand Reviewed but attach no result
        let ledger =
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path())
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
            evaluate_acceptance(&contract, clean_gate(), None, no_guard_trigger(), dir.path())
                .await;
        assert_eq!(ledger.status, AcceptanceStatus::Rejected);
        assert!(ledger.detail.expect("detail").contains("needs more work"));
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
}

// ================================================================================================
// C12: faithful port of the pi acceptance criteria/evidence model
// (`pi-subagents/src/runs/shared/acceptance.ts` + `src/shared/types.ts:248-385`).
//
// The enum-lattice API above (`AcceptanceStatus`/`AcceptanceContract`/`evaluate_acceptance`) is the
// crate's original acceptance subsystem, wired into `exec/mod.rs::run_sync`. This module is the
// diagnosed-missing C12 port: the criteria/evidence/report model, level inference
// (`inferLevel`/`resolveEffectiveAcceptance`, acceptance.ts:69-302), report-JSON parse+validate,
// evidence checks incl. the real `git status --short` no-staged-files check, `stripAcceptanceReport`
// (acceptance.ts:494-515), and the async `evaluateAcceptance` ledger with REAL `verify[]` subprocess
// execution. Names live under `model` so they never collide with the enum-lattice API's own
// `AcceptanceLedger`/`evaluate_acceptance`.
// ================================================================================================
pub mod model {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use serde_json::Value;

    use crate::exec::completion_guard::{any_word_boundary, word_boundary_contains};

    // --------------------------------------------------------------------------------------------
    // Enums (types.ts:248-373)
    // --------------------------------------------------------------------------------------------

    /// `AcceptanceLevel` (types.ts:248) — `auto` is the "infer" sentinel; every other variant is a
    /// concrete provenance level. Ordering rank is `none < attested < checked < verified < reviewed`
    /// ([`level_rank`]); `Auto` has no rank.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum AcceptanceLevel {
        Auto,
        None,
        Attested,
        Checked,
        Verified,
        Reviewed,
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
                AcceptanceLevel::Reviewed => "reviewed",
            }
        }
    }

    /// `LEVEL_RANK` (acceptance.ts:22-28) — `None` for `Auto` (unranked).
    fn level_rank(level: AcceptanceLevel) -> Option<u8> {
        match level {
            AcceptanceLevel::Auto => Option::None,
            AcceptanceLevel::None => Some(0),
            AcceptanceLevel::Attested => Some(1),
            AcceptanceLevel::Checked => Some(2),
            AcceptanceLevel::Verified => Some(3),
            AcceptanceLevel::Reviewed => Some(4),
        }
    }

    /// `AcceptanceEvidenceKind` (types.ts:250-259).
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

        fn from_wire(text: &str) -> Option<Self> {
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

    /// `"required" | "recommended"` (types.ts:265).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum GateSeverity {
        Required,
        Recommended,
    }

    // --------------------------------------------------------------------------------------------
    // Config-input shapes (types.ts:261-293)
    // --------------------------------------------------------------------------------------------

    /// One acceptance criterion as authored: either a bare `must` string or a full [`AcceptanceGate`]
    /// (types.ts `Array<string | AcceptanceGate>`).
    #[derive(Debug, Clone, PartialEq)]
    pub enum CriterionInput {
        Text(String),
        Gate(AcceptanceGate),
    }

    /// `AcceptanceGate` (types.ts:261-266).
    #[derive(Debug, Clone, PartialEq)]
    pub struct AcceptanceGate {
        pub id: Option<String>,
        pub must: Option<String>,
        pub evidence: Option<Vec<AcceptanceEvidenceKind>>,
        pub severity: Option<GateSeverity>,
    }

    /// `AcceptanceVerifyCommand` (types.ts:268-275).
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

    /// `AcceptanceReviewGate` (types.ts:277-281).
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

    /// `AcceptanceReviewGate | false` (types.ts:288) — `Disabled` is the `false` shorthand.
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(untagged)]
    pub enum ReviewSetting {
        Disabled(bool),
        Gate(AcceptanceReviewGate),
    }

    /// `AcceptanceConfig` (types.ts:283-291).
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

    /// `AcceptanceInput = AcceptanceLevel | false | AcceptanceConfig` (types.ts:293).
    #[derive(Debug, Clone, PartialEq)]
    pub enum AcceptanceInput {
        Level(AcceptanceLevel),
        Disabled,
        Config(AcceptanceConfig),
    }

    // --------------------------------------------------------------------------------------------
    // Resolved shapes (types.ts:295-312)
    // --------------------------------------------------------------------------------------------

    /// `ResolvedAcceptanceGate` (types.ts:295-300).
    #[derive(Debug, Clone, PartialEq)]
    pub struct ResolvedAcceptanceGate {
        pub id: String,
        pub must: String,
        pub evidence: Vec<AcceptanceEvidenceKind>,
        pub severity: GateSeverity,
    }

    /// `ResolvedAcceptanceConfig` (types.ts:302-312).
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
    // Report / runtime-check / ledger shapes (types.ts:314-385)
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

    /// `AcceptanceReport` (types.ts:314-334).
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

    /// `AcceptanceRuntimeCheck` (types.ts:338-342).
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

    /// `AcceptanceVerifyResult` (types.ts:344-353).
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
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum ReviewResultStatus {
        NoBlockers,
        Blockers,
        NeedsParentDecision,
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

    /// `AcceptanceReviewResult` (types.ts:355-363).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct AcceptanceReviewResult {
        pub status: ReviewResultStatus,
        pub findings: Vec<ReviewFinding>,
    }

    /// `AcceptanceLedgerStatus` (types.ts:365-373).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum AcceptanceLedgerStatus {
        NotRequired,
        Claimed,
        Attested,
        Checked,
        Verified,
        Reviewed,
        Accepted,
        Rejected,
    }

    /// `AcceptanceLedger` (types.ts:375-385, subset actually populated by `evaluateAcceptance`).
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcceptanceLedger {
        pub status: AcceptanceLedgerStatus,
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
            AcceptanceLevel::Verified | AcceptanceLevel::Reviewed => vec![
                ChangedFiles,
                TestsAdded,
                CommandsRun,
                ValidationOutput,
                ResidualRisks,
                NoStagedFiles,
            ],
        }
    }

    /// `SubagentRunMode` (types.ts:146) — carried for parity with pi's `inferLevel` input even
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

    /// `inferLevel` (acceptance.ts:69-125) — regex-free word-boundary port (the classifier reuses
    /// `completion_guard`'s already-tested `word_boundary_contains`, exactly as the enum-lattice
    /// `heuristic_default` reuses `expects_implementation_mutation`).
    fn infer_level(input: &AcceptanceResolveInput) -> InferredLevel {
        let agent = input.agent_name.to_lowercase();
        let task = input.task.as_deref().unwrap_or("").to_lowercase();
        let mut reasons: Vec<String> = Vec::new();

        // /\b(?:reviewer|scout|context-builder|researcher|analyst)\b/
        let read_only_agent = any_word_boundary(
            &agent,
            &["reviewer", "scout", "context-builder", "researcher", "analyst"],
        );
        // /\b(?:read[- ]only|review[- ]only|do not edit|don't edit|no edits|without edits|inspect|summari[sz]e)\b/
        let read_only_task = any_word_boundary(
            &task,
            &[
                "read only",
                "read-only",
                "review only",
                "review-only",
                "do not edit",
                "don't edit",
                "no edits",
                "without edits",
                "inspect",
                "summarise",
                "summarize",
            ],
        );
        // /\b(?:fix|implement|update|write|edit|modify|migrate|release|security|delete|remove|refactor|commit)\b/ || /\bworker\b/
        let write_task = any_word_boundary(
            &task,
            &[
                "fix", "implement", "update", "write", "edit", "modify", "migrate", "release",
                "security", "delete", "remove", "refactor", "commit",
            ],
        ) || word_boundary_contains(&agent, "worker");
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
        let risky = (input.is_async && write_task) || input.dynamic || input.dynamic_group || risky_task;

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
            return InferredLevel {
                level: AcceptanceLevel::Reviewed,
                reasons,
                criteria: vec![
                    CriterionInput::Text(
                        "Implement the requested change without widening scope".to_string(),
                    ),
                    CriterionInput::Text(
                        "Return evidence sufficient for an independent acceptance review".to_string(),
                    ),
                ],
                evidence: required_evidence_for_level(AcceptanceLevel::Reviewed),
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

    /// `normalizeAcceptanceInput` (acceptance.ts:127-132).
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

    /// `explicitAcceptanceCanDisable` (acceptance.ts:134-136).
    fn explicit_acceptance_can_disable(explicit: &AcceptanceConfig) -> bool {
        explicit.level == Some(AcceptanceLevel::None)
            && explicit
                .reason
                .as_deref()
                .is_some_and(|reason| !reason.trim().is_empty())
    }

    /// `normalizeCriteria` (acceptance.ts:251-263).
    fn normalize_criteria(
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

    fn unique_evidence(items: &[AcceptanceEvidenceKind]) -> Vec<AcceptanceEvidenceKind> {
        let mut seen: Vec<AcceptanceEvidenceKind> = Vec::new();
        for item in items {
            if !seen.contains(item) {
                seen.push(*item);
            }
        }
        seen
    }

    /// `resolveEffectiveAcceptance` (acceptance.ts:265-302) — including the explicit-vs-inferred MAX
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

        let mut review = if explicit.review.is_some() {
            explicit.review.clone()
        } else {
            inferred.review.clone()
        };
        // acceptance.ts:288-290: inference escalated the level to `reviewed` (explicit asked for
        // something lower, and set no review of its own) — downgrade the inferred required review to
        // optional so it is not an explicit hard blocker.
        if level == AcceptanceLevel::Reviewed
            && explicit_level != AcceptanceLevel::Auto
            && explicit_level != AcceptanceLevel::Reviewed
            && explicit.review.is_none()
            && let Some(ReviewSetting::Gate(gate)) = &review {
                review = Some(ReviewSetting::Gate(AcceptanceReviewGate {
                    required: Some(false),
                    ..gate.clone()
                }));
            }

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
    // formatAcceptancePrompt (acceptance.ts:304-348)
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

    /// `formatAcceptancePrompt` (acceptance.ts:304-348).
    #[must_use]
    pub fn format_acceptance_prompt(acceptance: &ResolvedAcceptanceConfig) -> String {
        if acceptance.level == AcceptanceLevel::None {
            return String::new();
        }
        let mut lines: Vec<String> = vec![
            String::new(),
            "## Acceptance Contract".to_string(),
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
    // Fenced-block scanning mirroring pi's regexes (acceptance.ts:350-373, 423-427, 494-515)
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

    /// `extractBalancedJson` (acceptance.ts:350-373).
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

    /// `parseReportJson` (acceptance.ts:409-421).
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

    /// `unwrapAcceptanceReport` (acceptance.ts:375-381).
    fn unwrap_acceptance_report(value: &Value) -> &Value {
        let Value::Object(map) = value else {
            return value;
        };
        if let Some(inner) = map.get("acceptance") {
            return inner;
        }
        if let Some(inner) = map.get("acceptance-report") {
            return inner;
        }
        value
    }

    /// `validationPathLabelForWrapper` (acceptance.ts:429-435).
    fn validation_path_label_for_wrapper(value: &Value) -> &'static str {
        let Value::Object(map) = value else {
            return "";
        };
        if map.contains_key("acceptance") {
            "acceptance"
        } else if map.contains_key("acceptance-report") {
            "acceptance-report"
        } else {
            ""
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

    fn is_string_array(value: Option<&Value>) -> bool {
        matches!(value, Some(Value::Array(items)) if items.iter().all(Value::is_string))
    }

    fn validate_string_array_field(errors: &mut Vec<String>, value: Option<&Value>, path: &str) {
        let Some(Value::Array(items)) = value else {
            push_type_error(errors, path, "string[]", value);
            return;
        };
        for (index, item) in items.iter().enumerate() {
            if !item.is_string() {
                push_type_error(errors, &format!("{path}[{index}]"), "string", Some(item));
            }
        }
    }

    /// `validateAcceptanceReport` (acceptance.ts:551-620). Returns `(report, errors)`; a present
    /// `report` requires no errors AND at least one recognized report field.
    fn validate_acceptance_report(
        value: &Value,
        path_label: &str,
    ) -> (Option<AcceptanceReport>, Vec<String>) {
        let mut errors: Vec<String> = Vec::new();
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
                for (index, item) in items.iter().enumerate() {
                    let ipath = format!("{cpath}[{index}]");
                    let Value::Object(obj) = item else {
                        push_type_error(&mut errors, &ipath, "object", Some(item));
                        continue;
                    };
                    if let Some(id) = obj.get("id")
                        && !id.is_string() {
                            push_type_error(&mut errors, &format!("{ipath}.id"), "string", Some(id));
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
                    if !obj.get("summary").is_some_and(Value::is_string) {
                        push_type_error(
                            &mut errors,
                            &format!("{ipath}.summary"),
                            "string",
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
        if let Some(diff) = map.get("diffSummary")
            && !diff.is_string() {
                push_type_error(
                    &mut errors,
                    &path_for(path_label, "diffSummary"),
                    "string",
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

    /// `hasGenericAcceptanceReportSignal` (acceptance.ts:393-407).
    fn has_generic_acceptance_report_signal(value: &Value) -> bool {
        let Value::Object(map) = value else {
            return false;
        };
        if !map.contains_key("criteriaSatisfied") {
            return false;
        }
        is_string_array(map.get("changedFiles"))
            || is_string_array(map.get("testsAddedOrUpdated"))
            || is_commands_run_array(map.get("commandsRun"))
            || is_string_array(map.get("validationOutput"))
            || is_string_array(map.get("residualRisks"))
            || map.get("noStagedFiles").is_some_and(Value::is_boolean)
            || map.get("diffSummary").is_some_and(Value::is_string)
            || is_string_array(map.get("reviewFindings"))
            || map.get("manualNotes").is_some_and(Value::is_string)
    }

    /// `isCommandsRunArray` (acceptance.ts:383-391).
    fn is_commands_run_array(value: Option<&Value>) -> bool {
        let Some(Value::Array(items)) = value else {
            return false;
        };
        items.iter().all(|item| {
            let Value::Object(obj) = item else {
                return false;
            };
            obj.get("command").is_some_and(Value::is_string)
                && matches!(
                    obj.get("result").and_then(Value::as_str),
                    Some("passed") | Some("failed") | Some("not-run")
                )
                && obj.get("summary").is_some_and(Value::is_string)
        })
    }

    // --------------------------------------------------------------------------------------------
    // parseAcceptanceReport / stripAcceptanceReport (acceptance.ts:437-515)
    // --------------------------------------------------------------------------------------------

    /// `parseAcceptanceReport` result (acceptance.ts:451).
    #[derive(Debug, Clone, PartialEq, Default)]
    pub struct ParsedAcceptanceReport {
        pub report: Option<AcceptanceReport>,
        pub error: Option<String>,
    }

    /// `parseAcceptanceReportBody` (acceptance.ts:437-441).
    fn parse_acceptance_report_body(body: &str) -> Result<(Option<AcceptanceReport>, Vec<String>), String> {
        let parsed = parse_report_json(body)?;
        let report = unwrap_acceptance_report(&parsed);
        let label = validation_path_label_for_wrapper(&parsed);
        Ok(validate_acceptance_report(report, label))
    }

    /// `parseGenericJsonAcceptanceReportBody` (acceptance.ts:443-449).
    fn parse_generic_json_acceptance_report_body(body: &str) -> Option<AcceptanceReport> {
        let parsed = parse_report_json(body).ok()?;
        let report = unwrap_acceptance_report(&parsed);
        let (validated, _errors) = validate_acceptance_report(report, "");
        let report = validated?;
        // Re-check the generic signal against the ORIGINAL value (pi checks `validation.report`,
        // which is the same object, minus dropped unknown fields — the signal keys are all known).
        if has_generic_acceptance_report_signal(report_to_value(&report).as_ref().unwrap_or(&Value::Null)) {
            Some(report)
        } else {
            Option::None
        }
    }

    fn report_to_value(report: &AcceptanceReport) -> Option<Value> {
        serde_json::to_value(report).ok()
    }

    /// `parseAcceptanceReport` (acceptance.ts:451-492).
    #[must_use]
    pub fn parse_acceptance_report(output: &str) -> ParsedAcceptanceReport {
        let fenced = fenced_block_bodies(output, &["acceptance-report"]);
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
        for body in fenced_block_bodies(output, &["json", "jsonc", "json5"]) {
            if let Some(report) = parse_generic_json_acceptance_report_body(&body) {
                return ParsedAcceptanceReport {
                    report: Some(report),
                    error: Option::None,
                };
            }
        }
        // ACCEPTANCE_REPORT: marker (acceptance.ts:473-490).
        if let Some(marker_index) = find_acceptance_report_marker(output)
            && let Some(json_start) = output.get(marker_index..).and_then(|s| s.find('{')).map(|r| marker_index + r)
                && let Some(json) = extract_balanced_json(output, json_start) {
                    match serde_json::from_str::<Value>(&json) {
                        Ok(parsed) => {
                            let report = unwrap_acceptance_report(&parsed);
                            let label = validation_path_label_for_wrapper(&parsed);
                            let (validated, errors) = validate_acceptance_report(report, label);
                            return match validated {
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
                            };
                        }
                        Err(err) => {
                            return ParsedAcceptanceReport {
                                report: Option::None,
                                error: Some(err.to_string()),
                            };
                        }
                    }
                }
        ParsedAcceptanceReport {
            report: Option::None,
            error: Some("Structured acceptance report not found.".to_string()),
        }
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

    /// `stripAcceptanceReport` (acceptance.ts:494-515). Removes a trailing `acceptance-report` /
    /// generic-JSON acceptance-report fence (and a trailing `ACCEPTANCE_REPORT: {...}` marker) from
    /// the DELIVERED output, so a caller sees the human answer, never the machine report JSON.
    #[must_use]
    pub fn strip_acceptance_report(output: &str) -> String {
        // The trailing-fence variant (`\n?```(tag)\s*\n([\s\S]*?)```\s*`) over all four tags.
        let tags = ["acceptance-report", "json", "jsonc", "json5"];
        let matches = fenced_matches(output, &tags, true, true);
        // The LAST match with only whitespace after it is the trailing fence (acceptance.ts:497-502).
        let trailing = matches.into_iter().rev().find(|m| {
            output
                .get(m.end..)
                .is_none_or(|tail| tail.trim().is_empty())
        });
        if let Some(fence) = trailing {
            if fence.tag == "acceptance-report" {
                return output.get(..fence.index).unwrap_or("").trim_end().to_string();
            }
            if parse_generic_json_acceptance_report_body(&fence.body).is_some() {
                return output.get(..fence.index).unwrap_or("").trim_end().to_string();
            }
        }
        // Fallbacks (acceptance.ts:511-514): a trailing acceptance-report fence, then a trailing
        // ACCEPTANCE_REPORT: {...} marker, then trimEnd.
        let stripped = strip_trailing_acceptance_report_fence(output);
        let stripped = strip_trailing_acceptance_marker(&stripped);
        stripped.trim_end().to_string()
    }

    /// `/\n?```acceptance-report\s*\n[\s\S]*?```\s*$/i` (acceptance.ts:512).
    fn strip_trailing_acceptance_report_fence(output: &str) -> String {
        let matches = fenced_matches(output, &["acceptance-report"], true, true);
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
    // Report-driven runtime checks (acceptance.ts:622-669)
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

    /// `checkCriteriaSatisfied` (acceptance.ts:622-630).
    fn check_criteria_satisfied(
        criteria: &[ResolvedAcceptanceGate],
        report: &AcceptanceReport,
    ) -> Vec<AcceptanceRuntimeCheck> {
        let reported: std::collections::HashMap<&str, &CriterionReport> = report
            .criteria_satisfied
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|item| item.id.as_deref().map(|id| (id, item)))
            .collect();
        criteria
            .iter()
            .filter(|criterion| criterion.severity != GateSeverity::Recommended)
            .map(|criterion| {
                let id = format!("criterion:{}", criterion.id);
                match reported.get(criterion.id.as_str()) {
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

    /// `checkNoStagedFiles` (acceptance.ts:646-655) — REAL `git status --short` subprocess in `cwd`.
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

    /// `runStructuralChecks` (acceptance.ts:657-669).
    async fn run_structural_checks(
        acceptance: &ResolvedAcceptanceConfig,
        report: &AcceptanceReport,
        cwd: &Path,
    ) -> Vec<AcceptanceRuntimeCheck> {
        let mut checks: Vec<AcceptanceRuntimeCheck> = Vec::new();
        for kind in &acceptance.evidence {
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
        if acceptance.evidence.contains(&AcceptanceEvidenceKind::NoStagedFiles) {
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

    /// `aggregateAcceptanceReport` (acceptance.ts:681-711).
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
            AcceptanceLedgerStatus::NotRequired => "not-required",
            AcceptanceLedgerStatus::Claimed => "claimed",
            AcceptanceLedgerStatus::Attested => "attested",
            AcceptanceLedgerStatus::Checked => "checked",
            AcceptanceLedgerStatus::Verified => "verified",
            AcceptanceLedgerStatus::Reviewed => "reviewed",
            AcceptanceLedgerStatus::Accepted => "accepted",
            AcceptanceLedgerStatus::Rejected => "rejected",
        }
    }

    // --------------------------------------------------------------------------------------------
    // runVerifyCommand (acceptance.ts:713-767) — REAL subprocess execution
    // --------------------------------------------------------------------------------------------

    const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 120_000;

    /// `runVerifyCommand` (acceptance.ts:713-767): execute one `verify[]` command as a REAL shell
    /// subprocess, observing its real exit code — never the child's own claim about it.
    async fn run_verify_command(
        command: &AcceptanceVerifyCommand,
        default_cwd: &Path,
    ) -> AcceptanceVerifyResult {
        let started = Instant::now();
        let cwd: PathBuf = match command.cwd.as_deref() {
            Some(rel) => {
                let p = Path::new(rel);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    default_cwd.join(p)
                }
            }
            Option::None => default_cwd.to_path_buf(),
        };
        let mut cmd = shell_command(&command.command);
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
                return AcceptanceVerifyResult {
                    id: command.id.clone(),
                    command: command.command.clone(),
                    cwd: cwd_str,
                    exit_code: Some(1),
                    status: if command.allow_failure == Some(true) {
                        VerifyRunStatus::AllowedFailure
                    } else {
                        VerifyRunStatus::Failed
                    },
                    stdout: Option::None,
                    stderr: Some(err.to_string()),
                    duration_ms: started.elapsed().as_millis(),
                };
            }
        };

        let timeout = Duration::from_millis(command.timeout_ms.unwrap_or(DEFAULT_VERIFY_TIMEOUT_MS));

        // Same shape as the enum-lattice API's `run_one_verify_command`: never race a
        // `self`-consuming `wait_with_output()` against the timeout, because the elapsed arm then
        // drops the only handle and abandons a live process group. Drain the pipes separately,
        // keep the `Child`, and kill on expiry (`abortVerification`, acceptance.ts:742-758).
        let stdout_task = child.stdout.take().map(super::spawn_pipe_drain);
        let stderr_task = child.stderr.take().map(super::spawn_pipe_drain);

        let waited = tokio::select! {
            biased;
            result = child.wait() => Some(result),
            () = tokio::time::sleep(timeout) => None,
        };

        let Some(waited) = waited else {
            let _ = crate::spawn::signal::terminate_on_timeout(&mut child).await;
            if let Some(task) = stdout_task {
                task.abort();
            }
            if let Some(task) = stderr_task {
                task.abort();
            }
            return AcceptanceVerifyResult {
                id: command.id.clone(),
                command: command.command.clone(),
                cwd: cwd_str,
                exit_code: Option::None,
                status: VerifyRunStatus::TimedOut,
                stdout: Option::None,
                stderr: Option::None,
                duration_ms: started.elapsed().as_millis(),
            };
        };

        let out_bytes = super::drained(stdout_task).await;
        let err_bytes = super::drained(stderr_task).await;

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
                AcceptanceVerifyResult {
                    id: command.id.clone(),
                    command: command.command.clone(),
                    cwd: cwd_str,
                    exit_code,
                    status,
                    stdout: trim_output(&String::from_utf8_lossy(&out_bytes)),
                    stderr: trim_output(&String::from_utf8_lossy(&err_bytes)),
                    duration_ms: started.elapsed().as_millis(),
                }
            }
            Err(err) => AcceptanceVerifyResult {
                id: command.id.clone(),
                command: command.command.clone(),
                cwd: cwd_str,
                exit_code: Some(1),
                status: if command.allow_failure == Some(true) {
                    VerifyRunStatus::AllowedFailure
                } else {
                    VerifyRunStatus::Failed
                },
                stdout: Option::None,
                stderr: Some(err.to_string()),
                duration_ms: started.elapsed().as_millis(),
            },
        }
    }

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

    // --------------------------------------------------------------------------------------------
    // evaluateAcceptance / acceptanceFailureMessage (acceptance.ts:769-856)
    // --------------------------------------------------------------------------------------------

    /// Input to [`evaluate_acceptance`] (acceptance.ts:769-775).
    pub struct EvaluateAcceptanceInput<'a> {
        pub acceptance: &'a ResolvedAcceptanceConfig,
        pub output: &'a str,
        pub cwd: &'a Path,
        pub report: Option<AcceptanceReport>,
        pub review_result: Option<AcceptanceReviewResult>,
    }

    /// `evaluateAcceptance` (acceptance.ts:769-845). Async because `verified` runs REAL `verify[]`
    /// subprocesses and `checked`/above may run the REAL `git status` no-staged-files check.
    #[must_use]
    pub async fn evaluate_acceptance(input: EvaluateAcceptanceInput<'_>) -> AcceptanceLedger {
        let acceptance = input.acceptance;
        let mut ledger = AcceptanceLedger {
            status: if acceptance.level == AcceptanceLevel::None {
                AcceptanceLedgerStatus::NotRequired
            } else {
                AcceptanceLedgerStatus::Claimed
            },
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

        let report = match input.report {
            Some(report) => Some(report),
            Option::None => {
                let parsed = parse_acceptance_report(input.output);
                if parsed.report.is_none() {
                    ledger.child_report_parse_error = parsed.error.clone();
                    ledger.runtime_checks.push(AcceptanceRuntimeCheck {
                        id: "attestation".to_string(),
                        status: RuntimeCheckStatus::Failed,
                        message: parsed
                            .error
                            .unwrap_or_else(|| "Structured acceptance report missing.".to_string()),
                    });
                    ledger.status = AcceptanceLedgerStatus::Rejected;
                    return ledger;
                }
                parsed.report
            }
        };
        let Some(report) = report else {
            // Unreachable: the `None` branch above already returned on a missing report.
            ledger.status = AcceptanceLedgerStatus::Rejected;
            return ledger;
        };
        ledger.child_report = Some(report.clone());
        ledger.status = AcceptanceLedgerStatus::Attested;

        let rank = level_rank(acceptance.level).unwrap_or(0);
        let checked_rank = 2u8;
        let verified_rank = 3u8;

        if rank >= checked_rank {
            let mut checks = check_criteria_satisfied(&acceptance.criteria, &report);
            checks.extend(run_structural_checks(acceptance, &report, input.cwd).await);
            ledger.runtime_checks = checks;
            if ledger
                .runtime_checks
                .iter()
                .any(|c| c.status == RuntimeCheckStatus::Failed)
            {
                ledger.status = AcceptanceLedgerStatus::Rejected;
                return ledger;
            }
            ledger.status = AcceptanceLedgerStatus::Checked;
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
                return ledger;
            }
            let mut runs = Vec::new();
            for command in &acceptance.verify {
                runs.push(run_verify_command(command, input.cwd).await);
            }
            ledger.verify_runs = runs;
            if ledger.verify_runs.iter().any(|run| {
                matches!(run.status, VerifyRunStatus::Failed | VerifyRunStatus::TimedOut)
            }) {
                ledger.status = AcceptanceLedgerStatus::Rejected;
                return ledger;
            }
            ledger.status = AcceptanceLedgerStatus::Verified;
        }

        if acceptance.level == AcceptanceLevel::Reviewed {
            if let Some(review) = input.review_result {
                let status = if review.status == ReviewResultStatus::NoBlockers {
                    AcceptanceLedgerStatus::Reviewed
                } else {
                    AcceptanceLedgerStatus::Rejected
                };
                ledger.review_result = Some(review);
                ledger.status = status;
            } else {
                let optional_review = matches!(
                    &acceptance.review,
                    Some(ReviewSetting::Gate(g)) if g.required == Some(false)
                );
                let severity = if acceptance.explicit && !optional_review {
                    ReviewFindingSeverity::Blocker
                } else {
                    ReviewFindingSeverity::NonBlocking
                };
                ledger.review_result = Some(AcceptanceReviewResult {
                    status: ReviewResultStatus::NeedsParentDecision,
                    findings: vec![ReviewFinding {
                        severity,
                        file: Option::None,
                        issue: "Reviewed acceptance requires an independent reviewer result."
                            .to_string(),
                        rationale: "The run cannot be marked reviewed from child evidence alone."
                            .to_string(),
                    }],
                });
                let review_disabled = matches!(&acceptance.review, Some(ReviewSetting::Disabled(false)));
                if review_disabled || (acceptance.explicit && !optional_review) {
                    ledger.status = AcceptanceLedgerStatus::Rejected;
                }
            }
        }

        ledger
    }

    /// `acceptanceFailureMessage` (acceptance.ts:847-856).
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
        match ledger.review_result.as_ref().map(|r| r.status) {
            Some(ReviewResultStatus::NeedsParentDecision) => {
                Some("Acceptance review required but no automatic reviewer result is available.".to_string())
            }
            Some(ReviewResultStatus::Blockers) => {
                Some("Acceptance review found blockers.".to_string())
            }
            _ => Some("Acceptance rejected.".to_string()),
        }
    }

    // --------------------------------------------------------------------------------------------
    // validateAcceptanceInput (acceptance.ts:138-249)
    // --------------------------------------------------------------------------------------------

    const VALID_LEVELS: &[&str] = &["auto", "none", "attested", "checked", "verified", "reviewed"];
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
            Value::String(s) => {
                if !VALID_LEVELS.contains(&s.as_str()) {
                    errors.push(format!("{path_label} has invalid level '{s}'."));
                }
                return errors;
            }
            Value::Object(_) => {}
            _ => {
                errors.push(format!(
                    "{path_label} must be a string level, false, or an object."
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
        if let Some(level) = map.get("level")
            && !level.as_str().is_some_and(|l| VALID_LEVELS.contains(&l)) {
                errors.push(format!(
                    "{path_label}.level must be one of auto, none, attested, checked, verified, reviewed."
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
        match map.get("evidence") {
            Some(Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    if item
                        .as_str()
                        .and_then(AcceptanceEvidenceKind::from_wire).is_none()
                    {
                        errors.push(format!(
                            "{path_label}.evidence[{index}] is not a supported evidence kind."
                        ));
                    }
                }
            }
            Some(_) => errors.push(format!("{path_label}.evidence must be an array.")),
            Option::None => {}
        }
        validate_verify_input(&mut errors, map.get("verify"), path_label);
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

    fn validate_criteria_input(errors: &mut Vec<String>, criteria: Option<&Value>, path_label: &str) {
        match criteria {
            Option::None => {}
            Some(Value::Array(items)) => {
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
                    if gate.get("id").and_then(Value::as_str).is_none_or(|i| i.trim().is_empty()) {
                        errors.push(format!("{cpath}.id is required."));
                    }
                    if gate.get("must").and_then(Value::as_str).is_none_or(|m| m.trim().is_empty()) {
                        errors.push(format!("{cpath}.must is required."));
                    }
                    match gate.get("evidence") {
                        Some(Value::Array(evs)) => {
                            for (ei, item) in evs.iter().enumerate() {
                                if item.as_str().and_then(AcceptanceEvidenceKind::from_wire).is_none() {
                                    errors.push(format!(
                                        "{cpath}.evidence[{ei}] is not a supported evidence kind."
                                    ));
                                }
                            }
                        }
                        Some(_) => errors.push(format!("{cpath}.evidence must be an array.")),
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

    fn validate_verify_input(errors: &mut Vec<String>, verify: Option<&Value>, path_label: &str) {
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
            Some(_) => errors.push(format!("{path_label}.verify must be an array.")),
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
            assert_eq!(
                resolve(AcceptanceResolveInput {
                    agent_name: "worker".into(),
                    task: Some("Implement the fix".into()),
                    is_async: true,
                    ..Default::default()
                })
                .level,
                AcceptanceLevel::Reviewed
            );
            assert_eq!(
                resolve(AcceptanceResolveInput {
                    agent_name: "worker".into(),
                    task: Some("Fix each item".into()),
                    mode: Some(SubagentRunMode::Chain),
                    dynamic: true,
                    ..Default::default()
                })
                .level,
                AcceptanceLevel::Reviewed
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
            let prompt = format_acceptance_prompt(&resolved);
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
            assert!(bad_review.error.as_deref().unwrap().contains("reviewFindings[0]: expected string; got object"));

            let bad_command = parse_acceptance_report(&report_text(
                json!({"commandsRun": [{"command": "npm test", "exitCode": 0}]}),
                "acceptance-report",
            ));
            let err = bad_command.error.as_deref().unwrap();
            assert!(err.contains("commandsRun[0].result: expected one of \"passed\", \"failed\", \"not-run\"; got missing"));
            assert!(err.contains("commandsRun[0].summary: expected string; got missing"));

            let bad_criteria = parse_acceptance_report(&report_text(
                json!({"criteriaSatisfied": [{"id": 7, "status": "done", "evidence": ""}]}),
                "acceptance-report",
            ));
            let err = bad_criteria.error.as_deref().unwrap();
            assert!(err.contains("criteriaSatisfied[0].id: expected string; got number 7"));
            assert!(err.contains("criteriaSatisfied[0].status: expected one of \"satisfied\", \"not-satisfied\", \"not-applicable\"; got \"done\""));
            assert!(err.contains("criteriaSatisfied[0].evidence: expected non-empty string; got \"\""));
        }

        // ---- evaluateAcceptance (async, real subprocess / real git) ----

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
                review_result: None,
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
                review_result: None,
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
                review_result: None,
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
                review_result: None,
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
        async fn reviewed_mode_records_reviewer_outcomes() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a risky fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Reviewed),
                    review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                        agent: Some("reviewer".into()),
                        focus: None,
                        required: Some(true),
                    })),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let no_blockers = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                review_result: Some(AcceptanceReviewResult {
                    status: ReviewResultStatus::NoBlockers,
                    findings: vec![],
                }),
            })
            .await;
            assert_eq!(no_blockers.status, AcceptanceLedgerStatus::Reviewed);

            let blockers = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                review_result: Some(AcceptanceReviewResult {
                    status: ReviewResultStatus::Blockers,
                    findings: vec![ReviewFinding {
                        severity: ReviewFindingSeverity::Blocker,
                        file: None,
                        issue: "Missing test".into(),
                        rationale: "Acceptance requires test evidence.".into(),
                    }],
                }),
            })
            .await;
            assert_eq!(blockers.status, AcceptanceLedgerStatus::Rejected);

            let unavailable = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                review_result: None,
            })
            .await;
            assert_eq!(unavailable.status, AcceptanceLedgerStatus::Rejected);
            assert_eq!(
                unavailable.review_result.as_ref().map(|r| r.status),
                Some(ReviewResultStatus::NeedsParentDecision)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn explicit_checked_is_not_an_explicit_reviewed_blocker_when_inference_recommends_review() {
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
            assert_eq!(acceptance.level, AcceptanceLevel::Reviewed);
            assert!(matches!(
                &acceptance.review,
                Some(ReviewSetting::Gate(g)) if g.required == Some(false)
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
                review_result: None,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Checked);
            assert_eq!(
                ledger.review_result.as_ref().map(|r| r.status),
                Some(ReviewResultStatus::NeedsParentDecision)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn does_not_mark_reviewed_without_an_independent_reviewer_result() {
            let dir = temp_dir();
            let acceptance = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement a fix".into()),
                explicit: cfg(AcceptanceConfig {
                    level: Some(AcceptanceLevel::Reviewed),
                    review: Some(ReviewSetting::Disabled(false)),
                    ..Default::default()
                }),
                ..Default::default()
            });
            assert_eq!(acceptance.level, AcceptanceLevel::Reviewed);
            let ledger = evaluate_acceptance(EvaluateAcceptanceInput {
                acceptance: &acceptance,
                output: &report_text(json!({}), "acceptance-report"),
                cwd: dir.path(),
                report: None,
                review_result: None,
            })
            .await;
            assert_eq!(ledger.status, AcceptanceLedgerStatus::Rejected);
            assert!(acceptance_failure_message(&ledger)
                .unwrap()
                .to_lowercase()
                .contains("review required"));
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
                review_result: None,
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
            assert!(v(json!({"evidence": ["bogus"]})).iter().any(|e| e.contains("acceptance.evidence[0] is not a supported evidence kind")));
            assert!(v(json!({"review": true})).iter().any(|e| e.contains("acceptance.review must be false or an object")));
            assert!(v(json!({"review": {"required": "yes"}})).iter().any(|e| e.contains("acceptance.review.required must be a boolean")));
            assert!(v(json!({"stopRules": [123]})).iter().any(|e| e.contains("acceptance.stopRules[0] must be a string")));
            assert!(v(json!({"surprise": true})).iter().any(|e| e.contains("acceptance.surprise is not supported")));
        }
    }
}

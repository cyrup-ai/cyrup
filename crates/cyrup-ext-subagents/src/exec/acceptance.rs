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

    let child = match cmd.spawn() {
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

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let exit_code = output.status.code();
            let passed = exit_code == Some(0);
            let mut combined = output.stdout;
            combined.extend_from_slice(&output.stderr);
            VerifyCommandResult {
                command: command.to_string(),
                exit_code,
                passed,
                output_tail: tail_utf8_lossy(&combined, VERIFY_OUTPUT_TAIL_BYTES),
                spawn_error: None,
            }
        }
        Ok(Err(err)) => VerifyCommandResult {
            command: command.to_string(),
            exit_code: None,
            passed: false,
            output_tail: String::new(),
            spawn_error: Some(format!("failed to wait on verify command: {err}")),
        },
        Err(_elapsed) => VerifyCommandResult {
            command: command.to_string(),
            exit_code: None,
            passed: false,
            output_tail: String::new(),
            spawn_error: Some(format!(
                "verify command exceeded its {}ms timeout and was abandoned",
                timeout.as_millis()
            )),
        },
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

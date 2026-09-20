//! The release verdicts and the by-run inspection.
//!
//! Ports pi `active-async-capacity.ts:50-68` (the verdict/inspection types), `:212-297` (the four
//! verdict functions) and `:311-336` (`inspectActiveAsyncCapacityOwner`) @`v0.66.0`, with the
//! process-terminal proof rung and the no-proof fallback ladder beneath it, both described on
//! [`runner_release_verdict`].

use std::path::{Path, PathBuf};

use crate::background::process_terminal::ProcessTerminalState;
use crate::background::reconcile::{Liveness, check_pid_identity_with};
use crate::background::{RunDir, RunId, RunMode, RunStatus};
use crate::identity::SessionId;
use crate::registration::AbandonedSlotRelease;

use super::config::CapacityOptions;
use super::key::{
    ActiveAsyncCapacityKind, ActiveAsyncCapacityOwner, capacity_session_dirs, occupied_slots,
    read_owner,
};

/// pi `ActiveAsyncCapacityReleaseEvidence` (`:50-56`) — the diagnostic record an
/// abandoned-timeout release appends to the run's own `events.jsonl`.
///
/// The three constant fields are upstream's literal string types, not values a caller chooses:
/// this evidence shape exists ONLY on the abandoned-timeout path, where by construction the
/// process proof is unknown and the runner pid is gone. Serialize-only — nothing in either
/// implementation reads the event back; it exists so an operator investigating "why did my slot
/// disappear" finds the answer beside the run it was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAsyncCapacityReleaseEvidence {
    /// Always `"abandoned-timeout"` (pi `:51`).
    pub released_by: &'static str,
    /// Always `"unknown"` (pi `:52`), and that is a TRUE statement on this path rather than a
    /// placeholder: the abandoned-timeout ladder is only reached when the run's own
    /// `process-terminal.json` was missing, `pending` or refused — a matching `observed` proof
    /// releases the slot several rungs earlier, on [`runner_release_verdict`]'s proof rung, and
    /// never reaches this evidence shape at all.
    pub process_proof: &'static str,
    /// Always `"gone"` (pi `:53`) — the ladder is unreachable unless the probe said
    /// [`Liveness::Dead`].
    pub runner_pid: &'static str,
    /// How long the run had been silent when the slot was taken (pi `:54`).
    pub last_activity_age_ms: i64,
    /// The threshold that was exceeded (pi `:55`).
    pub abandoned_slot_release_after_ms: i64,
}

impl ActiveAsyncCapacityReleaseEvidence {
    /// pi's object literal at `:251-257`.
    #[must_use]
    pub fn abandoned_timeout(last_activity_age_ms: i64, threshold_ms: i64) -> Self {
        Self {
            released_by: "abandoned-timeout",
            process_proof: "unknown",
            runner_pid: "gone",
            last_activity_age_ms,
            abandoned_slot_release_after_ms: threshold_ms,
        }
    }
}

/// pi `ActiveAsyncCapacityReleaseVerdict` (`:58-61`).
///
/// Three outcomes, not two: `NotOwned` is "this slot is not this run's to release" (it was
/// transferred away, or no slot records the run at all), which is categorically different from
/// "this run's slot is being kept".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveAsyncCapacityReleaseVerdict {
    /// The slot may be reclaimed. `evidence` is present only on the abandoned-timeout ladder.
    Releasable {
        /// Human-readable justification, rendered into the diagnostic event.
        reason: String,
        /// The abandoned-timeout record, when that is the ladder that fired.
        evidence: Option<ActiveAsyncCapacityReleaseEvidence>,
    },
    /// The slot is still this run's. **Every** rung that cannot positively prove release lands
    /// here — the cap failing closed costs an admission, the cap failing open costs correctness.
    Retained {
        /// Which rung retained it.
        reason: String,
    },
    /// The slot does not belong to the run that was asked about.
    NotOwned {
        /// Why not.
        reason: String,
    },
}

impl ActiveAsyncCapacityReleaseVerdict {
    /// `true` only for [`Self::Releasable`] — the single predicate reconciliation branches on.
    #[must_use]
    pub fn is_releasable(&self) -> bool {
        matches!(self, Self::Releasable { .. })
    }

    /// pi `release.state` (`:58-61`) — the literal `releasable` / `retained` / `not-owned` word
    /// `debug.run`'s `Active capacity:` line interpolates (`run-status.ts:61`).
    #[must_use]
    pub fn state_word(&self) -> &'static str {
        match self {
            Self::Releasable { .. } => "releasable",
            Self::Retained { .. } => "retained",
            Self::NotOwned { .. } => "not-owned",
        }
    }

    /// pi `release.reason` — the rung that decided the verdict, whichever variant carries it.
    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            Self::Releasable { reason, .. }
            | Self::Retained { reason }
            | Self::NotOwned { reason } => reason,
        }
    }

    fn retained(reason: impl Into<String>) -> Self {
        Self::Retained {
            reason: reason.into(),
        }
    }

    fn releasable(reason: impl Into<String>) -> Self {
        Self::Releasable {
            reason: reason.into(),
            evidence: None,
        }
    }
}

/// How the slot found by [`inspect_active_async_capacity_owner`] relates to the run asked about —
/// pi `ActiveAsyncCapacityInspection["relation"]` (`:65`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityRelation {
    /// The slot is held BY this run.
    Current,
    /// The slot was held by this run and has since been TRANSFERRED to another.
    Source,
    /// No slot in any scanned pool records this run.
    None,
}

impl CapacityRelation {
    /// pi's literal `"current" | "source" | "none"` (`:65`), as `debug.run`'s `Capacity owner:`
    /// line interpolates it (`run-status.ts:65`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Source => "source",
            Self::None => "none",
        }
    }
}

/// pi `ActiveAsyncCapacityInspection` (`:63-68`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveAsyncCapacityInspection {
    /// The owner record found, when one was.
    pub owner: Option<ActiveAsyncCapacityOwner>,
    /// How that owner relates to the run asked about.
    pub relation: CapacityRelation,
    /// The `slot-<n>` directory the owner was read from.
    pub slot_dir: Option<PathBuf>,
    /// What reconciliation would do with the slot right now.
    pub release: ActiveAsyncCapacityReleaseVerdict,
}

/// The `status.json` of a run directory, or `None` when it is missing OR unreadable — pi
/// `readStatus` (`shared/utils.ts`), which collapses both to `null`.
///
/// Every verdict below treats the two identically (`:217`'s single `if (!status)` rung), so the
/// distinction is deliberately erased here rather than propagated as a `Result` no caller branches
/// on.
pub(super) async fn read_run_status(async_dir: &Path) -> Option<RunStatus> {
    crate::background::control::read_status_file(&RunDir::for_existing(async_dir).status())
        .await
        .ok()
        .flatten()
}

/// pi `runnerReleaseVerdict` (`:216-236`) — the rung ladder that decides whether a terminated
/// run's capacity slot may be handed to someone else.
///
/// # The rungs, in order
///
/// 1..5 are upstream's identity and terminality guards (`:217-221`), unchanged.
///
/// 6. **The early-failure carve-out** (`:222-226`): a `not-started` proof on `status`, for THIS
///    run and THIS runner instance, beside a non-empty `status.error`. A run that failed before
///    its child ever started has no close to observe and must not wait out the abandoned timeout.
/// 7. **THE PROOF** (`:227-235`): the run's own `process-terminal.json`, read back and matched
///    against the owner's run id AND
///    [`ActiveAsyncCapacityOwner::runner_process_instance_id`]. `observed` with both matching is
///    upstream's single positive release proof, and it is consulted BEFORE the pid ladder below —
///    never the other way round. It is NOT the first rung of the ladder as a whole: rungs 1-5's
///    identity and terminality guards and rung 6's carve-out all answer before the sidecar is even
///    opened, and a rung added above this one must keep that order.
/// 8. **The no-proof fallback ladder — cyrup's, and it STAYS** (see below).
/// 9. [`abandoned_runner_release_verdict`], upstream's `:238-263` tail.
///
/// # Why the pid ladder remains beneath the proof
///
/// The proof is written by the RUNNER at its own close
/// ([`crate::background::runner_main`]'s tail), because cyrup's orchestrator detaches its runner
/// and drops the child handle unawaited — there is no parent left to observe the close
/// (R-SA-078). So a runner that was `SIGKILL`ed, OOM-killed, or lost with its machine writes NO
/// proof, ever. Deleting the fallback when the real proof landed would retain every such run's
/// slot forever, and after `limit` of them the session could never spawn again — which is
/// strictly worse than having no cap at all. The fallback is therefore not a substitute any more;
/// it is the answer to a genuinely different question, asked only when the first has no answer:
/// *the run is terminal, no proof exists, and the runner's pid is demonstrably gone.*
///
/// "Demonstrably gone" is [`crate::background::reconcile::check_pid_identity_with`], not bare
/// `kill(pid, 0)`: the owner records the pid's [`ProcessStartIdentity`] at the bind, so a RECYCLED
/// pid — live, but a different process — reads `Dead` here instead of holding the slot forever.
/// That recycled-pid hole is the defect ledger row VL-S4 named.
///
/// # One upstream method still has no cyrup call site
///
/// `rollbackBeforeRunnerProceed` (`:426-446`) re-binds a reservation during upstream's
/// runner-proceed HANDSHAKE. cyrup has no startup barrier — `spawn_detached_runner_with_command`
/// returns with the runner already running — so there is no window for it to cover. See
/// [`super::ActiveAsyncCapacityHandle`]'s own doc.
///
/// [`ProcessStartIdentity`]: crate::background::session_lease::ProcessStartIdentity
pub async fn runner_release_verdict(
    owner: &ActiveAsyncCapacityOwner,
    status: Option<&RunStatus>,
    options: &CapacityOptions,
) -> ActiveAsyncCapacityReleaseVerdict {
    // pi `:217`.
    let Some(status) = status else {
        return ActiveAsyncCapacityReleaseVerdict::retained("status file is missing or unreadable");
    };
    // pi `:218` — `!owner.runnerProcessInstanceId`. [`ActiveAsyncCapacityOwner::is_started`] is
    // the wide disjunct pi tests at `:180`: any bind field present means a runner was bound, which
    // keeps a slot written by a build older than the instance-id field from reading as unstarted.
    if !owner.is_started() {
        return ActiveAsyncCapacityReleaseVerdict::retained(
            "runner process identity has not been recorded",
        );
    }
    // pi `:219`. Typed here, so upstream's `status.sessionId ?? "unknown"` interpolation is an
    // `Option::map` rather than a nullish coalesce.
    if status.session_id.as_ref() != Some(&owner.owner_session_id) {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "status session {} does not match owner session {}",
            status
                .session_id
                .as_ref()
                .map_or("unknown", SessionId::as_str),
            owner.owner_session_id
        ));
    }
    // pi `:220`.
    if status.run_id != owner.run_id {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "status run {} does not match owner run {}",
            status.run_id, owner.run_id
        ));
    }
    // pi `:221` via `terminalState` (`:212-214`) — `state !== queued|running|paused`.
    // [`crate::background::RunState::is_terminal`] is byte-equal to it: `Paused` is retained by
    // BOTH, because a paused run is resumable and its runner may still be alive.
    if !status.state.is_terminal() {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "run is still {}",
            state_word(status.state)
        ));
    }
    // pi `:222-226` — the early-failure carve-out. A run that failed before its child ever
    // started HAS no close to observe, so the ladder below would never release it and it would sit
    // out the whole abandoned timeout for no information gained.
    if let Some(proof) = status.process_terminal.as_ref()
        && proof.state() == ProcessTerminalState::NotStarted
        && *proof.run_id() == owner.run_id
        && owner
            .runner_process_instance_id
            .as_ref()
            .is_some_and(|instance| proof.runner_process_instance_id() == instance)
        // pi `typeof status.error === "string" && status.error` (`:225`) — an EMPTY error string
        // does not satisfy it either, which `Option::is_some_and` reproduces exactly.
        && status.error.as_ref().is_some_and(|error| !error.is_empty())
    {
        return ActiveAsyncCapacityReleaseVerdict::releasable(
            "run failed before child startup completed",
        );
    }

    // pi `:227-235` — THE PROOF. It is the first rung to consult the run's own artifacts, and it
    // answers BEFORE the pid ladder beneath it; the guards at `:217-221` and the carve-out above
    // still run first. `read_process_terminal` runs
    // the sidecar through `validate_proof` with this expectation, so a proof left behind by a
    // DIFFERENT runner in a reused directory degrades to `unknown` rather than releasing this
    // run's slot.
    let proof = match owner.runner_process_instance_id.as_ref() {
        Some(instance) => {
            crate::background::process_terminal::read_process_terminal(
                &RunDir::for_existing(&owner.async_dir),
                crate::background::process_terminal::ProofExpectation::new(&owner.run_id, instance),
            )
            .await
        }
        // A slot written before the identity existed has nothing to match a proof against, and
        // `validate_proof`'s whole job is that match. Reading one anyway would accept a proof this
        // record cannot vouch for.
        None => None,
    };
    if let Some(proof) = proof.as_ref()
        && proof.state() == ProcessTerminalState::Observed
        && *proof.run_id() == owner.run_id
        && owner
            .runner_process_instance_id
            .as_ref()
            .is_some_and(|instance| proof.runner_process_instance_id() == instance)
    {
        return ActiveAsyncCapacityReleaseVerdict::releasable(
            "matching observed process-terminal proof is present",
        );
    }
    // pi `:235` — `proof?.state ?? "missing"`, the word the fallback ladder's every retained
    // reason interpolates.
    let proof_reason = format!(
        "process-terminal proof is {}",
        proof
            .as_ref()
            .map_or("missing", |proof| proof.state().as_str())
    );

    // The no-proof FALLBACK ladder — cyrup's own, and the reason it stays is on this function's
    // doc: a runner killed before it could write a proof would otherwise hold its slot forever.
    let proof_reason = match owner.runner_pid {
        Some(pid) => {
            // `check_pid_identity_with`, not a bare liveness probe: the owner recorded this pid's
            // start identity at the bind, so a RECYCLED pid answers `Dead` here rather than
            // holding the slot for the life of the machine. That is VL-S4's named defect, closed.
            let liveness = check_pid_identity_with(
                pid,
                owner.runner_process_start_identity.as_ref(),
                |pid| options.pid_liveness(pid),
                |pid| options.pid_start_identity(pid),
            );
            match liveness {
                Liveness::Dead => {
                    return ActiveAsyncCapacityReleaseVerdict::releasable(format!(
                        "{proof_reason}; runner pid {pid} is confirmed gone and the run is terminal"
                    ));
                }
                // NEVER read `Unknown` as dead (`background/reconcile.rs`'s `Liveness`): an
                // `EPERM`-class probe failure commonly means the process is alive and merely
                // unobservable, and reclaiming a live run's slot is the failure mode the whole cap
                // exists to avoid.
                liveness => format!(
                    "{proof_reason}; runner pid {pid} liveness is {}",
                    liveness_word(liveness)
                ),
            }
        }
        // The durable bind stamped `runnerStartedAt` but never landed the pid (pi's `markStarted`
        // comment at `:410-411` names exactly this window).
        None => format!("{proof_reason}; runner pid was never recorded"),
    };
    abandoned_runner_release_verdict(status, &proof_reason, options)
}

/// pi `abandonedRunnerReleaseVerdict` (`:238-263`) — the no-proof tail, reached when the owner's
/// own pid is alive, unobservable, or was never bound.
///
/// Every rung is upstream's, in upstream's order, over upstream's input: the ladder probes
/// **`status.pid`**, the pid the RUN recorded, which is a different field from the owner's
/// reservation pid the proof rung above consumes (upstream's own split: `owner
/// .runnerProcessInstanceId` at `:230` against `status.pid` at `:244`).
///
/// # One rung is unreachable in cyrup and is collapsed
///
/// Upstream's "last activity timestamp is missing or invalid" rung (`:247`) guards a
/// `lastActivityAt ?? lastUpdate ?? endedAt` chain in which every term is optional. cyrup's chain
/// is `telemetry.last_activity_at` (`Option<i64>`) `.unwrap_or(status.last_update)`, and
/// `last_update` is a NON-optional `i64` — so the chain always produces a value and the rung can
/// never fire. Collapsed to two rungs, exactly as `terminal_run_index/update.rs` already does for
/// the same `last_update` non-optionality.
#[must_use]
pub fn abandoned_runner_release_verdict(
    status: &RunStatus,
    proof_reason: &str,
    options: &CapacityOptions,
) -> ActiveAsyncCapacityReleaseVerdict {
    // pi `:241`.
    let threshold_ms = match options.abandoned_slot_release() {
        AbandonedSlotRelease::Never => {
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "{proof_reason}; abandoned-timeout policy is disabled"
            ));
        }
        AbandonedSlotRelease::After(ms) => ms,
    };
    // pi `:242`. A `Complete`/`Stopped` run with no proof is NEVER taken on this ladder — only a
    // failure is ambiguous enough to justify reclaiming a slot on a timer.
    if status.state != crate::background::RunState::Failed {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "{proof_reason}; abandoned-timeout policy requires a failed run, not {}",
            state_word(status.state)
        ));
    }
    // pi `:243`.
    let Some(pid) = status.pid.filter(|pid| *pid >= 1) else {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "{proof_reason}; runner PID is missing or invalid"
        ));
    };
    // pi `:244-245` — `liveness !== "dead"`, so BOTH `Alive` and `Unknown` retain.
    let liveness = options.pid_liveness(pid);
    if liveness != Liveness::Dead {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "{proof_reason}; runner PID liveness is {}",
            liveness_word(liveness)
        ));
    }
    // pi `:246` minus the unreachable `:247` rung (see the doc above).
    let last_activity_at = status
        .telemetry
        .last_activity_at
        .unwrap_or(status.last_update);
    let last_activity_age_ms = options.now().saturating_sub(last_activity_at).max(0);
    // pi `:250` — `<=`, so a run silent for EXACTLY the threshold is still retained.
    if last_activity_age_ms <= threshold_ms {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "{proof_reason}; last activity age {last_activity_age_ms}ms has not exceeded \
             abandoned-timeout {threshold_ms}ms"
        ));
    }
    let evidence =
        ActiveAsyncCapacityReleaseEvidence::abandoned_timeout(last_activity_age_ms, threshold_ms);
    ActiveAsyncCapacityReleaseVerdict::Releasable {
        // pi `:259` verbatim in shape.
        reason: format!(
            "{}: {proof_reason}; runner PID is gone; process proof unknown; last activity age \
             {last_activity_age_ms}ms exceeds {threshold_ms}ms",
            evidence.released_by
        ),
        evidence: Some(evidence),
    }
}

/// pi `workflowReleaseVerdict` (`:265-290` @`v0.66.0`, plus the `v0.68.0` not-started guard folded
/// in at `:283-286`).
///
/// # The one substitution, and the one fallback
///
/// 1. The live-controller check (`:271`) is
///    [`crate::extension::SubagentExecutor::live_workflow_run_ids`]'s set. A live workflow's slot
///    is NEVER reclaimed, which is why a leaked registry entry withholds capacity forever and a
///    missing one reclaims a live run's slot.
/// 2. Each async child's process-terminal proof (`:287-289`) is upstream's own, read off that
///    child's `process-terminal.json` and matched against the identity its `status.json`
///    published. Beneath it sits the same no-proof fallback [`runner_release_verdict`] keeps —
///    the child's recorded pid, confirmed gone — because a child killed before it could write a
///    proof would otherwise pin its parent's slot forever.
///
/// # Q5 — cyrup has no `step.async`, because cyrup has no async workflow child
///
/// Upstream hard-retains a workflow slot when `typeof step.async !== "boolean"` (`:274`), a
/// fail-closed guard against a status payload written by an older build. cyrup's [`StepStatus`]
/// carries no such classification, and NOT because the field was dropped: `route_workflow_mode`
/// refuses the async workflow shape outright (`extension/tool/routing.rs:598-599`), so every cyrup
/// workflow child is a FOREGROUND child of the workflow shell and upstream's `if (!step.async)
/// continue` applies to it. The classification is therefore not *missing*, it is constant `false`,
/// and the witness of the one shape that could be otherwise — a step that launched its own
/// addressable run — is [`crate::background::StepStatus::run_id`], upstream's own next guard
/// (`:276`). Porting `:274`'s fail-closed arm against an absent concept would retain EVERY
/// workflow slot forever, which is the same defect §D3 exists to prevent.
///
/// [`StepStatus`]: crate::background::StepStatus
pub async fn workflow_release_verdict(
    owner: &ActiveAsyncCapacityOwner,
    status: Option<&RunStatus>,
    options: &CapacityOptions,
) -> ActiveAsyncCapacityReleaseVerdict {
    // pi `:266`.
    let Some(status) = status else {
        return ActiveAsyncCapacityReleaseVerdict::retained("status file is missing or unreadable");
    };
    // pi `:267`.
    if status.session_id.as_ref() != Some(&owner.owner_session_id) {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "status session {} does not match owner session {}",
            status
                .session_id
                .as_ref()
                .map_or("unknown", SessionId::as_str),
            owner.owner_session_id
        ));
    }
    // pi `:268`.
    if status.run_id != owner.run_id {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "status run {} does not match owner run {}",
            status.run_id, owner.run_id
        ));
    }
    // pi `:269`.
    if status.mode != RunMode::Workflow {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "status mode is {}, not workflow",
            mode_word(status.mode)
        ));
    }
    // pi `:270`.
    if !status.state.is_terminal() {
        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
            "workflow is still {}",
            state_word(status.state)
        ));
    }
    // pi `:271` — the WORKFLOW_6 dependency.
    if options.live_workflow_run_ids().contains(&owner.run_id) {
        return ActiveAsyncCapacityReleaseVerdict::retained("workflow controller is still live");
    }
    for step in &status.steps {
        // pi `:273` — `step.workflowKey ?? step.agent`.
        let label = step
            .workflow_key
            .as_ref()
            .map_or_else(|| step.agent.clone(), ToString::to_string);
        // Q5, above: a step with no run id launched no addressable run, so it is upstream's
        // `!step.async` arm and cannot pin the parent's slot.
        let Some(child_run_id) = step.run_id.as_ref() else {
            continue;
        };
        // pi `:277-278` — the child directory is a SIBLING of the workflow's own run directory.
        let Some(child_dir) = owner
            .async_dir
            .parent()
            .map(|root| root.join(child_run_id.as_str()))
        else {
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "async workflow child {label} directory is missing"
            ));
        };
        if !tokio::fs::try_exists(&child_dir).await.unwrap_or(false) {
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "async workflow child {label} directory is missing"
            ));
        }
        // pi `:279-280`.
        let Some(child_status) = read_run_status(&child_dir).await else {
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "async workflow child {label} status is missing or unreadable"
            ));
        };
        // pi `:281`.
        if !child_status.state.is_terminal() {
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "async workflow child {label} is still {}",
                state_word(child_status.state)
            ));
        }
        // pi `:283-286` — a child that never started but recorded an error must NOT pin its
        // parent's slot, and upstream expresses "never started" through the child's own
        // `processTerminal.state === "not-started"`. That record now exists here, so the guard is
        // upstream's own, tested against the child's proof rather than against a substitute.
        //
        // The pid-absence disjunct is kept BESIDE it, not instead of it: a child launched by a
        // build older than the overlay field carries no `processTerminal` at all, and without the
        // second reading its parent's slot would be retained forever — which is the defect the
        // whole fallback ladder exists to prevent.
        let child_never_started = child_status.process_terminal.as_ref().is_some_and(|proof| {
            proof.state() == ProcessTerminalState::NotStarted && proof.run_id() == child_run_id
        }) || child_status.pid.is_none();
        if child_never_started
            && child_status
                .error
                .as_ref()
                .is_some_and(|error| !error.is_empty())
        {
            continue;
        }
        // pi `:282` — `!childStatus.processTerminal?.runnerProcessInstanceId`: no runner identity
        // at all, and no error to explain it. cyrup reads the identity off the child's own status
        // overlay, exactly as upstream does, and falls back to the recorded pid for a child
        // launched before the overlay field existed.
        let child_instance = child_status
            .process_terminal
            .as_ref()
            .map(crate::background::process_terminal::ProcessTerminal::runner_process_instance_id);
        if child_instance.is_none() && child_status.pid.is_none() {
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "async workflow child {label} has no runner process identity"
            ));
        }
        // pi `:287-289` — THE PROOF, the child's own `process-terminal.json`, matched against the
        // run id the parent's step recorded and the instance the child's status published.
        if let Some(instance) = child_instance {
            let proof = crate::background::process_terminal::read_process_terminal(
                &RunDir::for_existing(&child_dir),
                crate::background::process_terminal::ProofExpectation::new(child_run_id, instance),
            )
            .await;
            match proof {
                Some(proof)
                    if proof.state() == ProcessTerminalState::Observed
                        && proof.run_id() == child_run_id =>
                {
                    continue;
                }
                // pi `:288`'s retained reason, with pi's own `?? "missing"`.
                proof => {
                    let state = proof
                        .as_ref()
                        .map_or("missing", |proof| proof.state().as_str());
                    // The no-proof FALLBACK, for the same reason the runner verdict keeps one: a
                    // workflow child killed before it could write a proof would otherwise pin its
                    // parent's slot forever. A pid the kernel says is gone is the evidence.
                    if !child_pid_is_gone(&child_status, options) {
                        return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                            "async workflow child {label} process-terminal proof is {state}"
                        ));
                    }
                    continue;
                }
            }
        }
        // No published identity: the fallback ladder is all there is.
        if !child_pid_is_gone(&child_status, options) {
            let child_pid = child_status.pid.unwrap_or_default();
            return ActiveAsyncCapacityReleaseVerdict::retained(format!(
                "async workflow child {label} runner pid {child_pid} is not confirmed gone"
            ));
        }
    }
    // pi `:291`.
    ActiveAsyncCapacityReleaseVerdict::releasable(
        "workflow is terminal, controller is gone, and async children have observed proof or confirmed-gone runners",
    )
}

/// The workflow loop's no-proof fallback rung, spelled once — the same ladder
/// [`runner_release_verdict`] falls to, over the CHILD's recorded pid.
///
/// A child's `status.json` carries a pid but no start identity (that pair lives on the capacity
/// OWNER record, and a workflow child holds no slot of its own), so this rung is bare liveness and
/// is honest about it: `Dead` only, never [`Liveness::Unknown`].
fn child_pid_is_gone(child_status: &RunStatus, options: &CapacityOptions) -> bool {
    child_status
        .pid
        .is_some_and(|pid| options.pid_liveness(pid) == Liveness::Dead)
}

/// pi `ownerReleaseVerdict` (`:292-297`) — reads the owner's own `status.json` once and dispatches
/// on [`ActiveAsyncCapacityKind`].
pub async fn owner_release_verdict(
    owner: &ActiveAsyncCapacityOwner,
    options: &CapacityOptions,
) -> ActiveAsyncCapacityReleaseVerdict {
    let status = read_run_status(&owner.async_dir).await;
    match owner.kind {
        ActiveAsyncCapacityKind::Runner => {
            runner_release_verdict(owner, status.as_ref(), options).await
        }
        ActiveAsyncCapacityKind::Workflow => {
            workflow_release_verdict(owner, status.as_ref(), options).await
        }
    }
}

/// pi `inspectActiveAsyncCapacityOwner` (`:311-336`): which slot, in which pool, records `run_id`
/// — and what reconciliation would do with it right now.
///
/// `session_id` narrows the scan to one pool; `None` scans every pool under the root, which is how
/// a caller that knows only a run id finds its slot. `async_dir` is the second identity upstream
/// matches on (`:318`), so a run whose id was rewritten is still found by its directory.
///
/// # Errors
///
/// Any directory-listing failure other than "not found" — a pool we cannot list is not a pool we
/// may report as empty.
pub async fn inspect_active_async_capacity_owner(
    run_id: &RunId,
    session_id: Option<&SessionId>,
    async_dir: Option<&Path>,
    options: &CapacityOptions,
) -> std::io::Result<ActiveAsyncCapacityInspection> {
    for pool_dir in capacity_session_dirs(options.root_dir(), session_id).await? {
        for dir in occupied_slots(&pool_dir).await? {
            let Some(owner) = read_owner(&dir).await else {
                continue;
            };
            // pi `:318` — the run id OR the resolved async dir.
            let same_run = &owner.run_id == run_id
                || async_dir.is_some_and(|requested| paths_equal(&owner.async_dir, requested));
            let source_run = owner.source_run_id.as_ref() == Some(run_id);
            if !same_run && !source_run {
                continue;
            }
            if source_run && !same_run {
                // pi `:321-328`. The slot exists but is no longer this run's — a categorically
                // different answer from "retained", and the reason `transfer`'s `sourceRunId`
                // breadcrumb is written at all.
                return Ok(ActiveAsyncCapacityInspection {
                    relation: CapacityRelation::Source,
                    slot_dir: Some(dir),
                    release: ActiveAsyncCapacityReleaseVerdict::NotOwned {
                        reason: format!("slot was transferred to {}", owner.run_id),
                    },
                    owner: Some(owner),
                });
            }
            let release = owner_release_verdict(&owner, options).await;
            return Ok(ActiveAsyncCapacityInspection {
                owner: Some(owner),
                relation: CapacityRelation::Current,
                slot_dir: Some(dir),
                release,
            });
        }
    }
    // pi `:335`.
    Ok(ActiveAsyncCapacityInspection {
        owner: None,
        relation: CapacityRelation::None,
        slot_dir: None,
        release: ActiveAsyncCapacityReleaseVerdict::NotOwned {
            reason: "no active-capacity slot records this run".to_string(),
        },
    })
}

/// pi's `path.resolve(a) === path.resolve(b)` (`:318`).
///
/// [`std::fs::canonicalize`] is deliberately NOT used: it touches the filesystem and fails for a
/// directory that has already been removed, which is precisely the state a reconciling caller is
/// asking about. Upstream's `path.resolve` is likewise pure lexical normalization.
fn paths_equal(a: &Path, b: &Path) -> bool {
    a.components().eq(b.components())
}

/// The lowercase word upstream interpolates for a state (`status.state` is a string union there).
fn state_word(state: crate::background::RunState) -> &'static str {
    use crate::background::RunState;
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::Paused => "paused",
        RunState::Complete => "complete",
        RunState::Failed => "failed",
        RunState::Stopped => "stopped",
    }
}

/// The lowercase word upstream interpolates for a run mode.
fn mode_word(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Single => "single",
        RunMode::Parallel => "parallel",
        RunMode::Chain => "chain",
        RunMode::Workflow => "workflow",
    }
}

/// pi's `PidLiveness` string union (`stale-run-reconciler.ts`), which its reason strings
/// interpolate directly.
pub(crate) fn liveness_word(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Alive => "alive",
        Liveness::Dead => "dead",
        Liveness::Unknown => "unknown",
    }
}

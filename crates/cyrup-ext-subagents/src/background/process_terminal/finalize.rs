//! The ladder — pi `process-terminal.ts:144-160`, `:201-241`, `:243-310` @v0.68.0.
//!
//! See [the module doc](super) for the rung table. Two things in here are easy to mis-port and
//! each has a test that fails loudly if they are:
//!
//! * [`overlay_status`]'s per-step state derivation (upstream `:233`, one dense ternary). Its
//!   `expected === 0 => "not-started"` head is what makes a step that never spawned a child read
//!   as not-started instead of unknown, and its `records.length === expected` term is the only
//!   place a per-step proof can say "this step's writers are all accounted for".
//! * the `durable` flag's ordering (`:299-309`). The sidecar write comes FIRST and nothing else
//!   happens unless it succeeded.

use std::collections::BTreeMap;
use std::path::Path;

use crate::background::atomic::write_atomic_json;
use crate::background::reconcile::{Liveness, check_pid_liveness};
use crate::background::session_lease::{
    ProcessStartIdentity, SessionLeaseState, canonical_session_id, demonstrably_stale,
    inspect_session_lease, process_start_identity,
};
use crate::background::{RunDir, RunId, RunStatus};
use crate::jsonl::RunEventLog;

use super::candidate::read_process_terminal_candidate;
use super::proof::{ProofExpectation, read_process_terminal, unknown_proof};
use super::types::{
    CanonicalSessionTerminal, LeaseDisposition, ProcessInstanceExit, ProcessTerminal,
    ProcessTerminalBase, ProcessTerminalCandidate, ProcessTerminalReason, ProcessTerminalState,
    ResumeDisposition, RunnerCloseObservation,
};
use super::{PROCESS_TERMINAL_EVENT_TYPE, SUBAGENT_LIFECYCLE_ARTIFACT_VERSION};

/// pi `resumeDisposition` (`:144-148`) — can the run this proof describes be revived?
///
/// `state` is the run's (or the step's) own status word as it is spelled on disk. Upstream accepts
/// BOTH `"complete"` and `"completed"`; cyrup's [`RunState`](crate::background::RunState)
/// serializes the first and [`StepState`](crate::background::StepState) the first as well, but the
/// second is kept in the accept set because a status written by upstream — or by an older cyrup
/// build — must read the same way.
#[must_use]
pub fn resume_disposition(state: Option<&str>, session_file: Option<&Path>) -> ResumeDisposition {
    match state {
        // pi `:145` — a STOPPED run was ended on purpose and is never revived.
        Some("stopped") => ResumeDisposition::NonResumable,
        Some("complete" | "completed" | "failed" | "paused") => {
            // pi `:147` — the session file must actually still be on disk. A terminal run whose
            // transcript was swept is not resumable, however terminal it looks.
            if session_file.is_some_and(Path::exists) {
                ResumeDisposition::Resumable
            } else {
                ResumeDisposition::Unavailable
            }
        }
        // pi `:146` — anything still running, queued, or unreadable.
        _ => ResumeDisposition::Unavailable,
    }
}

/// pi `sessionProjection` (`:150-159`) — the observed proof's statement about the session file.
///
/// Built ONLY when both halves hold: the lease reads FREE, and (when this run held a revival
/// lease) its release was ACKNOWLEDGED. Either one missing yields `None` and the observed proof
/// carries no `canonicalSession` block at all — which is honest: the block's `freeAtObservation`
/// is a literal `true`, so its PRESENCE is the claim, and a block written over a still-leased file
/// would be a false one.
fn session_projection(
    candidate: &ProcessTerminalCandidate,
    lease: &SessionLeaseState,
) -> Option<CanonicalSessionTerminal> {
    let session_file = candidate.session_file.as_ref()?;
    if !lease.is_free() {
        return None;
    }
    if candidate.revival_lease_token.is_some()
        && candidate.revival_lease_release_acknowledged != Some(true)
    {
        return None;
    }
    let held_lease = candidate.revival_lease_token.is_some();
    Some(CanonicalSessionTerminal {
        canonical_session_id: canonical_session_id(session_file).ok()?,
        lease_disposition: if held_lease {
            LeaseDisposition::Released
        } else {
            LeaseDisposition::NotHeld
        },
        free_at_observation: true,
        canonical_session_lease_released: held_lease.then_some(true),
    })
}

/// pi `stepProcessTerminalProof` (`:201-222`) — one step's slice of the run-level proof.
fn step_process_terminal_proof(
    proof: &ProcessTerminal,
    child_index: usize,
    state: ProcessTerminalState,
    records: Vec<ProcessInstanceExit>,
    resume: ResumeDisposition,
) -> ProcessTerminal {
    let base = ProcessTerminalBase {
        version: proof.base().version,
        run_id: proof.run_id().clone(),
        child_index: Some(child_index),
        runner_process_instance_id: proof.runner_process_instance_id().clone(),
        resume_disposition: Some(resume),
    };
    match state {
        ProcessTerminalState::Observed => ProcessTerminal::Observed {
            base,
            // pi `:216` — a step's observation is timestamped from the RUN's proof when that proof
            // is itself observed, and from the clock only when it is not (which can only happen
            // through a caller that asks for an observed step under a non-observed run).
            observed_at: match proof {
                ProcessTerminal::Observed { observed_at, .. } => *observed_at,
                _ => crate::time::now_epoch_millis(),
            },
            instances: records,
            canonical_session: None,
        },
        ProcessTerminalState::Unknown => ProcessTerminal::Unknown {
            base,
            // pi `:219` — the run's own reason when it has one, and `writer-close-unverified`
            // otherwise: a step can only be unknown-under-a-non-unknown-run because its writers
            // did not add up.
            reason: proof
                .reason()
                .unwrap_or(ProcessTerminalReason::WriterCloseUnverified),
            diagnostic: None,
        },
        ProcessTerminalState::Pending => ProcessTerminal::Pending { base },
        ProcessTerminalState::NotStarted => ProcessTerminal::NotStarted { base },
    }
}

/// pi `overlayStatus` (`:224-241`) — copy the proof onto `status.processTerminal` and onto every
/// `status.steps[i].processTerminal`.
///
/// **Swallows every failure**, exactly as upstream does (`:238-240`: *"The proof sidecar remains
/// authoritative when terminal status is unavailable."*). A status file that cannot be read or
/// written does not invalidate the proof that was just persisted beside it.
pub async fn overlay_status(
    run_dir: &RunDir,
    proof: &ProcessTerminal,
    candidate: Option<&ProcessTerminalCandidate>,
) {
    let status_path = run_dir.status();
    let Ok(raw) = tokio::fs::read(&status_path).await else {
        return;
    };
    let Ok(mut status) = serde_json::from_slice::<RunStatus>(&raw) else {
        return;
    };
    status.process_terminal = Some(proof.clone());
    for (index, step) in status.steps.iter_mut().enumerate() {
        let key = index.to_string();
        let records = candidate
            .and_then(|candidate| candidate.writers.get(&key))
            .cloned()
            .unwrap_or_default();
        // pi `:232` — the declared count when the candidate declared one, else the RECORDED count
        // (which makes the two agree by construction, so an undeclared step never reads as
        // inconsistent).
        let expected = candidate
            .and_then(|candidate| candidate.expected_writers.as_ref())
            .and_then(|expected| expected.get(&key))
            .copied()
            .unwrap_or_else(|| u32::try_from(records.len()).unwrap_or(u32::MAX));
        // pi `:233`, one dense ternary, in its own order:
        //   expected === 0                                        -> "not-started"
        //   proof observed && records.length === expected         -> "observed"
        //   proof pending                                         -> "pending"
        //   otherwise                                             -> "unknown"
        // The head is what makes a step that never launched a child read as NOT-STARTED rather
        // than as an unproven one, and it is why `initialize_process_terminal`'s empty candidate
        // leaves every step not-started until the runner declares real counts.
        let step_state = if expected == 0 {
            ProcessTerminalState::NotStarted
        } else if proof.state() == ProcessTerminalState::Observed
            && u32::try_from(records.len()).unwrap_or(u32::MAX) == expected
        {
            ProcessTerminalState::Observed
        } else if proof.state() == ProcessTerminalState::Pending {
            ProcessTerminalState::Pending
        } else {
            ProcessTerminalState::Unknown
        };
        // pi `:234` — the step's OWN session file wins over the run-level one on the candidate.
        let session_file = step
            .session_file
            .as_deref()
            .or_else(|| candidate.and_then(|candidate| candidate.session_file.as_deref()));
        let resume = resume_disposition(Some(step.status.as_wire_word()), session_file);
        step.process_terminal = Some(step_process_terminal_proof(
            proof, index, step_state, records, resume,
        ));
    }
    let _ = write_atomic_json(&status_path, &status).await;
}

/// The ambient facts the lease rung's staleness ladder reads — this machine's hostname and the
/// two pid probes — bundled so a test can present a lease owner it could not otherwise own.
///
/// Same shape and same reason as
/// [`SessionLeaseOptions`](crate::background::session_lease::SessionLeaseOptions)'s last three
/// fields (pi `session-lease.ts:52-55`): a test cannot be running on a foreign hostname, cannot
/// hold a recycled pid, and cannot make its own pid dead, so it hands the ladder those facts
/// instead. [`Default`] is production and is what [`finalize_process_terminal`] uses.
#[derive(Clone, Debug)]
pub struct LeaseClaimProbe {
    /// This machine's hostname — rung 1 of the ladder, which stops at a FOREIGN host.
    pub hostname: String,
    /// `kill(pid, 0)` — rungs 2 and 4.
    pub liveness: fn(u32) -> Liveness,
    /// `/proc/<pid>/stat` field 20 — the recycled-pid half of rungs 2 and 4.
    pub start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
}

impl Default for LeaseClaimProbe {
    fn default() -> Self {
        Self {
            hostname: crate::background::async_retention::machine_hostname(),
            liveness: check_pid_liveness,
            start_identity_of: process_start_identity,
        }
    }
}

/// `[CYRUP-DELTA]` — does this lease record still CLAIM anything?
///
/// pi's rung is `session.state !== "free"` (`process-terminal.ts:273`) and nothing more, so a
/// lease DIRECTORY left behind by a runner that was `SIGKILL`ed refuses every later close of every
/// run that names the same session file. Upstream never reaches that state because its candidate
/// names `sessionFile` only on the revival path (`subagent-runner.ts:5182`) and a revival always
/// runs the staleness ladder first — but cyrup's candidate names it for every run that has one
/// (see `runner_main/entry.rs`'s delta), so upstream's rung WOULD be reachable here on a healthy,
/// never-revived run. It is not, because the question the rung is actually asking is *"is
/// something still writing this session file"*, and a record whose owner is
/// [`demonstrably_stale`] — the same four rungs
/// [`acquire_session_lease`](crate::background::session_lease::acquire_session_lease) reclaims a
/// lease on (pi `session-lease.ts:174-181`) — answers no. The very next revival will move that
/// directory aside as a tombstone; a proof that refused over it would be refusing over a claim
/// upstream's own protocol has already declared void.
///
/// Every rung of that ladder fails CLOSED, so the directions this can get wrong are the safe
/// ones: a foreign host is never stale, an unreadable owner is never stale (there is no record to
/// test, so the refusal stands as [`ProcessTerminalReason::CanonicalSessionUnavailable`]), and a
/// live pid is never stale.
///
/// A stale lease is NOT promoted to free for [`session_projection`]: the directory is still there,
/// so `freeAtObservation: true` would be a false claim. Such a run reaches `observed` with no
/// `canonicalSession` block, which is exactly the honesty rule that block's presence encodes.
fn lease_claim_is_live(lease: &SessionLeaseState, probe: &LeaseClaimProbe) -> bool {
    match lease {
        SessionLeaseState::Free { .. } => false,
        SessionLeaseState::Unreadable { .. } => true,
        SessionLeaseState::Owned { owner, .. } => !demonstrably_stale(
            owner,
            &probe.hostname,
            probe.liveness,
            probe.start_identity_of,
        ),
    }
}

/// pi `finalizeProcessTerminal` (`:243-310`) — the whole decision ladder, in upstream's order.
///
/// `lease_root` is the session-lease root this run's canonical session file is keyed under
/// ([`session_leases_root_in`](crate::background::session_leases_root_in) in production, a
/// `Roots::sandboxed` tempdir in every test). It is REQUIRED and has no default, so no test can
/// reach the shared machine-wide root by omission.
///
/// `events` is the run's `events.jsonl` writer, or `None` when the run has none. The
/// `subagent.run.process_terminal` line is appended through it ONLY on a durable write — see the
/// module doc.
pub async fn finalize_process_terminal(
    run_dir: &RunDir,
    run_id: &RunId,
    close: &RunnerCloseObservation,
    lease_root: &Path,
    events: &mut Option<RunEventLog>,
) -> ProcessTerminal {
    finalize_process_terminal_with(
        run_dir,
        run_id,
        close,
        lease_root,
        events,
        &LeaseClaimProbe::default(),
    )
    .await
}

/// [`finalize_process_terminal`] with the lease rung's ambient probes supplied — the
/// `check_pid_identity_with` idiom this crate already uses for every pid ladder.
///
/// Production calls [`finalize_process_terminal`]; this exists so a test can present a lease owner
/// on a foreign host, or one whose pid is dead, without owning either.
pub async fn finalize_process_terminal_with(
    run_dir: &RunDir,
    run_id: &RunId,
    close: &RunnerCloseObservation,
    lease_root: &Path,
    events: &mut Option<RunEventLog>,
    probe: &LeaseClaimProbe,
) -> ProcessTerminal {
    let expectation = ProofExpectation::new(run_id, &close.process_instance_id);
    // pi `:248-252` — an existing answer wins. An `observed` proof for THIS run and THIS runner is
    // returned verbatim; so is ANY `unknown` proof, because re-running the ladder over the same
    // inputs cannot produce a better answer and would double-emit the lifecycle event. A `pending`
    // proof (the one `initialize_process_terminal` wrote) deliberately does NOT short-circuit.
    if let Some(existing) = read_process_terminal(run_dir, expectation).await
        && tokio::fs::try_exists(run_dir.process_terminal())
            .await
            .unwrap_or(false)
    {
        match &existing {
            ProcessTerminal::Observed { .. }
                if existing.run_id() == run_id
                    && *existing.runner_process_instance_id() == close.process_instance_id =>
            {
                return existing;
            }
            ProcessTerminal::Unknown { .. } => return existing,
            _ => {}
        }
    }

    let (proof, candidate_for_overlay) =
        build_proof(run_dir, run_id, close, lease_root, probe).await;

    // pi `:299-309` — durability first, and NOTHING else happens without it.
    let mut durable = false;
    if write_atomic_json(&run_dir.process_terminal(), &proof)
        .await
        .is_ok()
    {
        durable = true;
        // pi `:303` — the active-run marker is released on the POSITIVE proof only. Any other
        // verdict leaves it in place for the age-based sweep, because an unproven close is not a
        // close.
        if matches!(proof, ProcessTerminal::Observed { .. }) {
            let _ =
                crate::background::active_run_index::release_active_run_index(run_dir.as_path())
                    .await;
        }
        overlay_status(run_dir, &proof, candidate_for_overlay.as_ref()).await;
        append_process_terminal_event(events, run_id, &proof).await;
    }
    if durable {
        proof
    } else {
        // pi `:309` — the returned value says the proof is not on disk, so a caller that acts on
        // the return value alone still cannot believe a proof no reader will find.
        unknown_proof(
            run_id.clone(),
            close.process_instance_id.clone(),
            ProcessTerminalReason::ProofWriteFailed,
            Some("Failed to persist process-terminal proof.".to_string()),
        )
    }
}

/// pi `:253-298` — the ladder itself, split out so [`finalize_process_terminal`]'s durability
/// block reads as the one thing it is. Returns the proof and the candidate the overlay needs.
async fn build_proof(
    run_dir: &RunDir,
    run_id: &RunId,
    close: &RunnerCloseObservation,
    lease_root: &Path,
    probe: &LeaseClaimProbe,
) -> (ProcessTerminal, Option<ProcessTerminalCandidate>) {
    let refuse = |reason: ProcessTerminalReason| {
        unknown_proof(
            run_id.clone(),
            close.process_instance_id.clone(),
            reason,
            None,
        )
    };

    // pi `:296-297` — any failure reading the candidate is itself an answer, carrying the refusal
    // sentence as the diagnostic.
    let candidate = match read_process_terminal_candidate(run_dir).await {
        Ok(candidate) => candidate,
        Err(error) => {
            return (
                unknown_proof(
                    run_id.clone(),
                    close.process_instance_id.clone(),
                    ProcessTerminalReason::ProofWriteFailed,
                    Some(error.to_string()),
                ),
                None,
            );
        }
    };
    // pi `:258`.
    let Some(candidate) = candidate else {
        return (refuse(ProcessTerminalReason::RunnerCandidateMissing), None);
    };
    // pi `:259` — the identity check the uuid mint exists for.
    if candidate.run_id != *run_id
        || candidate.runner_process_instance_id != close.process_instance_id
    {
        return (
            refuse(ProcessTerminalReason::RunnerInstanceMismatch),
            Some(candidate),
        );
    }

    let all_writers: Vec<ProcessInstanceExit> =
        candidate.writers.values().flatten().cloned().collect();
    // pi `:262-264` — the status is read for `resumeDisposition` alone, and a missing or
    // unreadable one is simply absent.
    let status = tokio::fs::read(run_dir.status())
        .await
        .ok()
        .and_then(|raw| serde_json::from_slice::<RunStatus>(&raw).ok());
    // pi `:265` — the lease is inspected only when the candidate names a session file.
    //
    // `[CYRUP-DELTA]` — the realpath failure is SPLIT, where upstream has one answer for both.
    // `inspectSessionLease` opens with `realpathSync.native` (`session-lease.ts:111`) and lets its
    // throw escape all the way into `:296`'s catch, so ANY resolution failure — including the
    // session file simply not being there — becomes `unknown` / `proof-write-failed`. Here:
    //
    // * `NotFound` is an ANSWER, not a failure. A path that does not resolve has no canonical id,
    //   so it has no lease directory, so nothing claims it. Refusing a cleanly closed run because
    //   its transcript was swept (retention, a cleaned worktree, an operator `rm`) would put
    //   `unknown (proof-write-failed)` on `debug.run`, hold the capacity slot behind the pid
    //   ladder, and hold the active-run marker for 24 hours — for a run whose every process was
    //   observed gone. That is upstream's `catch` being coarse, not upstream's contract, and
    //   cyrup reaches it far more often because its candidate names a session file on EVERY run.
    // * every other errno (`EACCES`, `ELOOP`, `EIO`, …) keeps upstream's answer exactly: the
    //   inspection could not be PERFORMED, so the lease state is unknown, so the proof is
    //   refused with the errno as its diagnostic.
    let lease = match candidate.session_file.as_ref() {
        Some(session_file) => match inspect_session_lease(session_file, lease_root) {
            Ok(lease) => Some(lease),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return (
                    unknown_proof(
                        run_id.clone(),
                        close.process_instance_id.clone(),
                        ProcessTerminalReason::ProofWriteFailed,
                        Some(error.to_string()),
                    ),
                    Some(candidate),
                );
            }
        },
        None => None,
    };

    // pi `:266-272` — the declared and recorded writer maps must agree, index for index, in BOTH
    // directions: a recorded index that was never declared (or whose count differs) is
    // inconsistent, and so is a declared index with a NON-ZERO count that recorded nothing.
    let expected: BTreeMap<String, u32> = candidate.expected_writers.clone().unwrap_or_else(|| {
        candidate
            .writers
            .iter()
            .map(|(index, records)| {
                (
                    index.clone(),
                    u32::try_from(records.len()).unwrap_or(u32::MAX),
                )
            })
            .collect()
    });
    let inconsistent_writers = candidate.writers.iter().any(|(index, records)| {
        expected.get(index) != Some(&u32::try_from(records.len()).unwrap_or(u32::MAX))
    }) || expected
        .iter()
        .any(|(index, count)| !candidate.writers.contains_key(index) && *count != 0);

    // pi `:273-274` — a lease that is still CLAIMED is the strongest refusal on the ladder: the
    // session file this run wrote is still being written, so the run's processes cannot be
    // declared gone. "Still claimed" is [`lease_claim_is_live`], not upstream's bare
    // `state !== "free"` — see that function for the delta and why the difference only ever moves
    // the answer in the direction upstream's own reclaim protocol already sanctions.
    let proof = if let Some(lease) = lease.as_ref().filter(|lease| lease_claim_is_live(lease, probe))
    {
        refuse(match lease {
            SessionLeaseState::Owned { .. } => ProcessTerminalReason::CanonicalSessionLeaseActive,
            _ => ProcessTerminalReason::CanonicalSessionUnavailable,
        })
    // pi `:275-276` — a revival lease whose release was never acknowledged.
    } else if candidate.revival_lease_token.is_some()
        && candidate.revival_lease_release_acknowledged != Some(true)
    {
        refuse(ProcessTerminalReason::CanonicalSessionReleaseUnverified)
    // pi `:277` — the inconsistency rung AND the crash discriminator. The second disjunct is
    // `allWriters.length === 0 && expectedEntries.length === 0`, which is precisely the candidate
    // `initialize_process_terminal` writes: a runner that died before declaring anything.
    } else if inconsistent_writers || (all_writers.is_empty() && expected.is_empty()) {
        refuse(ProcessTerminalReason::WriterCloseUnverified)
    // pi `:279` — a writer whose process TREE was not observed torn down. Vacuous upstream, where
    // `allWriters` is always empty; live here, where every entry describes a real OS child.
    } else if all_writers.iter().any(|writer| {
        matches!(writer, ProcessInstanceExit::PiWriter { process_tree, .. } if !process_tree.is_observed())
    }) {
        refuse(ProcessTerminalReason::ProcessTreeUnverified)
    } else {
        // pi `:282-293` — the positive proof.
        let runner = ProcessInstanceExit::Runner {
            process_instance_id: close.process_instance_id.clone(),
            close_observed_at: close.close_observed_at,
            exit_code: close.exit_code,
            signal: close.signal.clone(),
        };
        let mut instances = Vec::with_capacity(all_writers.len() + 1);
        instances.push(runner);
        instances.extend(all_writers);
        let canonical_session = lease
            .as_ref()
            .and_then(|lease| session_projection(&candidate, lease));
        let session_file = candidate
            .session_file
            .as_deref()
            .or_else(|| status.as_ref().and_then(|status| status.session_file.as_deref()));
        ProcessTerminal::Observed {
            base: ProcessTerminalBase {
                version: candidate.version,
                run_id: run_id.clone(),
                child_index: None,
                runner_process_instance_id: close.process_instance_id.clone(),
                resume_disposition: Some(resume_disposition(
                    status.as_ref().map(|status| status.state.as_wire_word()),
                    session_file,
                )),
            },
            observed_at: close.close_observed_at,
            instances,
            canonical_session,
        }
    };
    (proof, Some(candidate))
}

/// pi `:305` — one `events.jsonl` line, stamped with the lifecycle artifact version so a consumer
/// tailing across an upgrade can tell a shape it knows from one it must ignore.
///
/// Best-effort, like every other writer of this file: an event log is a diagnostic aid (R-SA-093),
/// never part of the durability contract. The line is only ever reached on a durable proof write,
/// which is the point of the ordering.
async fn append_process_terminal_event(
    events: &mut Option<RunEventLog>,
    run_id: &RunId,
    proof: &ProcessTerminal,
) {
    let Some(writer) = events.as_mut() else {
        return;
    };
    let line = serde_json::json!({
        "type": PROCESS_TERMINAL_EVENT_TYPE,
        "lifecycleArtifactVersion": SUBAGENT_LIFECYCLE_ARTIFACT_VERSION,
        "ts": crate::time::now_epoch_millis(),
        "runId": run_id.as_str(),
        "processTerminal": proof,
    })
    .to_string();
    let _ = writer.write_line(&line).await;
}

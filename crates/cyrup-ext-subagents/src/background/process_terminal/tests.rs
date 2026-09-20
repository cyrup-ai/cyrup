//! The ladder, rung by rung — pi `process-terminal.ts:243-310` @v0.68.0.
//!
//! Every test here drives [`finalize_process_terminal`] against a real run directory, because the
//! ladder's whole value is that each rung is a DIFFERENT answer: a test that only asserted "not
//! observed" would pass against an implementation that collapsed nine distinguishable verdicts
//! into one.
//!
//! Lease roots are always a per-test tempdir. `session_leases_root_in` is production's only
//! resolver and no test names it — see that function's doc for why that rule exists.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;
use crate::background::{RunDir, RunId, RunMode, RunState, RunStatus, StepStatus};
use crate::jsonl::BoundedJsonlWriter;

/// A run directory plus the two roots the ladder reads, all under one tempdir.
struct Fixture {
    _tmp: tempfile::TempDir,
    run_dir: RunDir,
    lease_root: PathBuf,
    run_id: RunId,
    instance: RunnerProcessInstanceId,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run_dir = tmp.path().join("async").join("run-1");
        std::fs::create_dir_all(&run_dir).expect("mkdir run dir");
        let lease_root = tmp.path().join("session-leases");
        Self {
            _tmp: tmp,
            run_dir: RunDir::for_existing(&run_dir),
            lease_root,
            run_id: RunId::from_token("run-1"),
            instance: RunnerProcessInstanceId::from_token("instance-1"),
        }
    }

    fn close(&self) -> RunnerCloseObservation {
        RunnerCloseObservation {
            process_instance_id: self.instance.clone(),
            close_observed_at: 1_700_000_000_123,
            exit_code: Some(0),
            signal: None,
        }
    }

    /// A terminal `status.json`, so `resumeDisposition` and the per-step overlay have something to
    /// read — every production finalize runs after `finish_run` has written exactly this.
    async fn write_status(&self, steps: usize, session_file: Option<&Path>) {
        let mut status = RunStatus::queued(self.run_id.clone(), RunMode::Chain, Some(4242));
        status.state = RunState::Complete;
        status.session_file = session_file.map(Path::to_path_buf);
        status.steps = (0..steps)
            .map(|i| StepStatus::pending(format!("a{i}")))
            .collect();
        for step in &mut status.steps {
            step.status = crate::background::StepState::Complete;
        }
        crate::background::atomic::write_atomic_json(&self.run_dir.status(), &status)
            .await
            .expect("write status");
    }

    async fn events(&self) -> Option<BoundedJsonlWriter> {
        Some(
            BoundedJsonlWriter::create(&self.run_dir.events())
                .await
                .expect("open events"),
        )
    }

    fn event_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.run_dir.events())
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("event line is JSON"))
            .collect()
    }

    fn process_terminal_events(&self) -> Vec<serde_json::Value> {
        self.event_lines()
            .into_iter()
            .filter(|line| line["type"] == json!(PROCESS_TERMINAL_EVENT_TYPE))
            .collect()
    }

    async fn finalize(&self) -> ProcessTerminal {
        let mut events = self.events().await;
        finalize_process_terminal(
            &self.run_dir,
            &self.run_id,
            &self.close(),
            &self.lease_root,
            &mut events,
        )
        .await
    }

    /// [`Self::finalize`] with the lease rung's staleness probes presented, so a test can own a
    /// hostname it is not running on and a pid that is not alive.
    async fn finalize_with(&self, probe: &LeaseClaimProbe) -> ProcessTerminal {
        let mut events = self.events().await;
        finalize_process_terminal_with(
            &self.run_dir,
            &self.run_id,
            &self.close(),
            &self.lease_root,
            &mut events,
            probe,
        )
        .await
    }
}

/// An observed pi-writer exit whose process GROUP was verified torn down — the shape cyrup's own
/// children really produce (`spawn/signal.rs::verify_process_group_terminated`).
fn observed_writer(attempt: u32) -> ProcessInstanceExit {
    ProcessInstanceExit::PiWriter {
        process_instance_id: format!("writer-{attempt}"),
        attempt,
        close_observed_at: 1_700_000_000_000,
        exit_code: Some(0),
        signal: None,
        process_tree: ProcessTreeTerminal::ObservedProcessGroup {
            process_group_id: 4242,
            verified_at: 1_700_000_000_001,
        },
    }
}

fn candidate_with(
    run_id: &RunId,
    instance: &RunnerProcessInstanceId,
    writers: BTreeMap<String, Vec<ProcessInstanceExit>>,
    expected: Option<BTreeMap<String, u32>>,
) -> ProcessTerminalCandidate {
    ProcessTerminalCandidate {
        version: ProcessTerminalVersion,
        run_id: run_id.clone(),
        runner_process_instance_id: instance.clone(),
        writers,
        expected_writers: expected,
        session_file: None,
        revival_lease_token: None,
        revival_lease_release_acknowledged: None,
    }
}

/// One step, one launched child, its close observed — the candidate a clean single-step cyrup run
/// really writes.
fn one_clean_step(run_id: &RunId, instance: &RunnerProcessInstanceId) -> ProcessTerminalCandidate {
    candidate_with(
        run_id,
        instance,
        BTreeMap::from([("0".to_string(), vec![observed_writer(0)])]),
        Some(BTreeMap::from([("0".to_string(), 1)])),
    )
}

// -------------------------------------------------------------------------------------------
// The launch write
// -------------------------------------------------------------------------------------------

/// pi `initializeProcessTerminal` (`:118-132`). The candidate it writes is EMPTY on purpose: that
/// emptiness is what makes a runner that died before declaring anything land on
/// `writer-close-unverified` instead of quietly reading as observed.
#[tokio::test]
async fn initialize_writes_an_empty_candidate_and_a_pending_proof() {
    let fx = Fixture::new();
    initialize_process_terminal(&fx.run_dir, &fx.run_id, &fx.instance)
        .await
        .expect("initialize");

    let candidate = read_process_terminal_candidate(&fx.run_dir)
        .await
        .expect("candidate reads")
        .expect("candidate exists");
    assert!(candidate.writers.is_empty(), "{candidate:?}");
    assert!(candidate.expected_writers.is_none(), "{candidate:?}");
    assert_eq!(candidate.run_id, fx.run_id);
    assert_eq!(candidate.runner_process_instance_id, fx.instance);

    let proof = read_process_terminal(&fx.run_dir, ProofExpectation::new(&fx.run_id, &fx.instance))
        .await
        .expect("proof exists");
    assert_eq!(proof.state(), ProcessTerminalState::Pending);
    assert_eq!(proof.run_id(), &fx.run_id);

    // 0600 — the candidate can carry a session-transcript path (pi `:114-116`).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(fx.run_dir.process_terminal_candidate())
            .expect("stat candidate")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the candidate must not be world-readable");
    }
}

// -------------------------------------------------------------------------------------------
// The ladder — one test per reason arm, in upstream's order
// -------------------------------------------------------------------------------------------

/// pi `:258`. A runner killed before it wrote its candidate must NOT read as cleanly closed: its
/// capacity slot would be released while a zombie may still hold the session file.
#[tokio::test]
async fn finalize_without_a_candidate_is_runner_candidate_missing() {
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    let proof = fx.finalize().await;
    assert_eq!(
        proof.reason(),
        Some(ProcessTerminalReason::RunnerCandidateMissing)
    );
    assert_eq!(proof.state(), ProcessTerminalState::Unknown);
}

/// pi `:259`. This is the arm the whole `RunnerProcessInstanceId` mint exists to make meaningful:
/// a proof written by run A must never be accepted as run B's.
#[tokio::test]
async fn finalize_with_a_mismatched_instance_is_runner_instance_mismatch() {
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    let foreign = RunnerProcessInstanceId::from_token("someone-elses-runner");
    write_process_terminal_candidate(&fx.run_dir, &one_clean_step(&fx.run_id, &foreign))
        .await
        .expect("write candidate");
    let proof = fx.finalize().await;
    assert_eq!(
        proof.reason(),
        Some(ProcessTerminalReason::RunnerInstanceMismatch)
    );

    // ...and the same refusal for a candidate belonging to another RUN.
    let fx2 = Fixture::new();
    fx2.write_status(1, None).await;
    write_process_terminal_candidate(
        &fx2.run_dir,
        &one_clean_step(&RunId::from_token("some-other-run"), &fx2.instance),
    )
    .await
    .expect("write candidate");
    assert_eq!(
        fx2.finalize().await.reason(),
        Some(ProcessTerminalReason::RunnerInstanceMismatch)
    );
}

/// pi `:277`'s second disjunct, from the NON-firing side — the direction that proves it is a
/// conjunction and not `allWriters.length === 0`.
///
/// The shape is upstream's own candidate verbatim (`subagent-runner.ts:5169-5174`, which writes
/// `writers[i] = []` and `expectedWriters[i] = 0` for every step because its children run
/// in-process), and it is reachable in cyrup for two ordinary reasons:
/// `runner_main::write_own_process_terminal_candidate` seeds an explicit zero-count entry for
/// every declared flat step, and a step only ever records a writer once `SpawnedChild::spawn`
/// SUCCEEDS — so a step whose agent binary does not exist, or a run whose every step was skipped,
/// produces exactly `writers = {"0": []}` with `expectedWriters = {"0": 0}`.
///
/// That run closed cleanly and must read `observed`. Weakening the rung to a bare
/// `all_writers.is_empty()` — which reads natural, and matches a skim of upstream's
/// `allWriters.length === 0` — would finalize every such run as `writer-close-unverified`:
/// `debug.run` would print `Sidecar process terminal: unknown (writer-close-unverified)` for a
/// clean close, `runner_release_verdict`'s proof rung would stop firing and fall through to the
/// pid ladder, and `read_live_active_run_ids` would hold the run's active marker for the full 24
/// hours instead of releasing it on the next read. Nothing else in the suite notices that change;
/// this test is the only thing between it and shipping.
#[tokio::test]
async fn finalize_reaches_observed_over_the_upstream_empty_writer_shape() {
    // (a) one declared step that launched nothing.
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    write_process_terminal_candidate(
        &fx.run_dir,
        &candidate_with(
            &fx.run_id,
            &fx.instance,
            BTreeMap::from([("0".to_string(), Vec::new())]),
            Some(BTreeMap::from([("0".to_string(), 0)])),
        ),
    )
    .await
    .expect("write candidate");
    let proof = fx.finalize().await;
    assert_eq!(
        proof.state(),
        ProcessTerminalState::Observed,
        "a declared step that launched no child is a CLEAN close, not an unproven one: {proof:?}"
    );
    // The positive proof still names the runner, and nothing else — there were no writers.
    match &proof {
        ProcessTerminal::Observed { instances, .. } => {
            assert_eq!(instances.len(), 1, "{instances:?}");
            assert!(
                matches!(&instances[0], ProcessInstanceExit::Runner { .. }),
                "{instances:?}"
            );
        }
        other => panic!("{other:?}"),
    }

    // (b) several declared steps, all skipped — the whole-run shape, and the one that made the
    // difference between this rung and `all_writers.is_empty()` invisible to the rest of the
    // suite.
    let all_skipped = Fixture::new();
    all_skipped.write_status(3, None).await;
    write_process_terminal_candidate(
        &all_skipped.run_dir,
        &candidate_with(
            &all_skipped.run_id,
            &all_skipped.instance,
            (0..3)
                .map(|i| (i.to_string(), Vec::new()))
                .collect::<BTreeMap<_, _>>(),
            Some((0..3).map(|i| (i.to_string(), 0)).collect()),
        ),
    )
    .await
    .expect("write candidate");
    assert_eq!(
        all_skipped.finalize().await.state(),
        ProcessTerminalState::Observed,
        "a run whose every step was skipped closed cleanly"
    );

    // The COUNTER-rung, so this test cannot pass against an implementation that dropped the
    // disjunct altogether: the same empty `writers` with NO `expectedWriters` at all is
    // `initialize_process_terminal`'s crash shape and stays refused.
    let crashed = Fixture::new();
    crashed.write_status(1, None).await;
    write_process_terminal_candidate(
        &crashed.run_dir,
        &candidate_with(&crashed.run_id, &crashed.instance, BTreeMap::new(), None),
    )
    .await
    .expect("write candidate");
    assert_eq!(
        crashed.finalize().await.reason(),
        Some(ProcessTerminalReason::WriterCloseUnverified),
        "a runner that declared nothing at all is still unproven"
    );
}

/// pi `:273-274`, both arms. A run whose session file is still CLAIMED must not be declared
/// terminally observed — `sessionProjection` would then report `freeAtObservation: true` about a
/// file that is not free.
///
/// "Claimed" is not "live", and the fixture says which this is: the owner it writes sits on host
/// `"here"`, which is not this machine, so rung 1 of `demonstrablyStale` (`session-lease.ts:175`)
/// stops the ladder outright and the claim stands whatever pid 4242 is doing. That is the
/// fail-closed direction — this machine's pid table says nothing about another machine's. The
/// case where the owner IS demonstrably stale is
/// [`finalize_over_a_stale_lease_reaches_observed`], and the two together are the rung.
#[tokio::test]
async fn finalize_with_a_held_lease_refuses_on_the_matching_arm() {
    for (owner_json, expected) in [
        (
            Some(json!({
                "version": 1,
                "token": "tok-1",
                "canonicalSessionFile": "/tmp/s.jsonl",
                "runId": "successor",
                "sourceRunId": "run-1",
                "pid": 4242,
                "hostname": "here",
                "writerState": "none",
                "acquiredAt": "2026-09-20T00:00:00.000Z",
                "acquiredAtMs": 1_700_000_000_000_i64,
                "updatedAtMs": 1_700_000_000_000_i64,
            })),
            ProcessTerminalReason::CanonicalSessionLeaseActive,
        ),
        (None, ProcessTerminalReason::CanonicalSessionUnavailable),
    ] {
        let fx = Fixture::new();
        fx.write_status(1, None).await;
        let session_file = fx.run_dir.as_path().join("session.jsonl");
        std::fs::write(&session_file, b"{}\n").expect("write session file");

        let mut candidate = one_clean_step(&fx.run_id, &fx.instance);
        candidate.session_file = Some(session_file.clone());
        write_process_terminal_candidate(&fx.run_dir, &candidate)
            .await
            .expect("write candidate");

        // A lease DIRECTORY is the claim; a readable `owner.json` inside it makes the lease
        // `owned`, an unreadable one makes it `unreadable`, and the two are different verdicts.
        let lease_dir =
            crate::background::session_lease::session_lease_dir(&session_file, &fx.lease_root)
                .expect("lease dir");
        std::fs::create_dir_all(&lease_dir).expect("create lease dir");
        if let Some(owner) = owner_json {
            std::fs::write(
                lease_dir.join("owner.json"),
                serde_json::to_vec(&owner).expect("encode owner"),
            )
            .expect("write owner");
        }

        assert_eq!(fx.finalize().await.reason(), Some(expected));
    }
}

/// `[CYRUP-DELTA]` — a lease directory whose owner is DEMONSTRABLY STALE claims nothing, and a
/// healthy run that names the same session file closes `observed` over it.
///
/// The state is ordinary, not exotic. Run A is revived; its runner takes the lease on transcript S
/// and is then `SIGKILL`ed, so `<lease_root>/<sha256(S)>/owner.json` survives (there is no `Drop`
/// — `session_lease/mod.rs` says the NEXT revival reclaims it). If no revival follows, that
/// directory sits there. cyrup's candidate names `session_file` on EVERY run that has one
/// (`runner_main/entry.rs`'s delta), so under upstream's bare `state !== "free"` rung every later
/// ordinary fork of S would close `unknown / canonical-session-lease-active` — `debug.run` would
/// say so, `runner_release_verdict`'s proof rung would stop firing, and the active-run marker
/// would be held for 24 hours — for runs whose every process was observed gone.
///
/// The counter-rung is in the same test: the SAME lease with a LIVE owner still refuses.
#[tokio::test]
async fn finalize_over_a_stale_lease_reaches_observed() {
    use crate::background::reconcile::Liveness;
    use crate::background::session_lease::ProcessStartIdentity;

    // This machine, by the probe's own reckoning — rung 1 stops on a foreign host, so a test that
    // wants rungs 2-4 to speak has to present the owner's host as its own.
    const HOST: &str = "this-machine";

    let owner = |writer_state: &str| {
        json!({
            "version": 1,
            "token": "tok-stale",
            "canonicalSessionFile": "/tmp/s.jsonl",
            "runId": "the-killed-revival",
            "sourceRunId": "run-0",
            "pid": 4242,
            "hostname": HOST,
            "writerState": writer_state,
            "acquiredAt": "2026-09-20T00:00:00.000Z",
            "acquiredAtMs": 1_700_000_000_000_i64,
            "updatedAtMs": 1_700_000_000_000_i64,
        })
    };

    for (label, liveness, expected_state, expected_reason) in [
        (
            "the owner's pid is confirmed gone",
            (|_| Liveness::Dead) as fn(u32) -> Liveness,
            ProcessTerminalState::Observed,
            None,
        ),
        (
            "the owner's pid is alive",
            (|_| Liveness::Alive) as fn(u32) -> Liveness,
            ProcessTerminalState::Unknown,
            Some(ProcessTerminalReason::CanonicalSessionLeaseActive),
        ),
    ] {
        let fx = Fixture::new();
        fx.write_status(1, None).await;
        let session_file = fx.run_dir.as_path().join("session.jsonl");
        std::fs::write(&session_file, b"{}\n").expect("write session file");

        let mut candidate = one_clean_step(&fx.run_id, &fx.instance);
        candidate.session_file = Some(session_file.clone());
        write_process_terminal_candidate(&fx.run_dir, &candidate)
            .await
            .expect("write candidate");

        let lease_dir =
            crate::background::session_lease::session_lease_dir(&session_file, &fx.lease_root)
                .expect("lease dir");
        std::fs::create_dir_all(&lease_dir).expect("create lease dir");
        std::fs::write(
            lease_dir.join("owner.json"),
            serde_json::to_vec(&owner("none")).expect("encode owner"),
        )
        .expect("write owner");

        let probe = LeaseClaimProbe {
            hostname: HOST.to_string(),
            liveness,
            start_identity_of: |_| None::<ProcessStartIdentity>,
        };
        let proof = fx.finalize_with(&probe).await;
        assert_eq!(proof.state(), expected_state, "{label}: {proof:?}");
        assert_eq!(proof.reason(), expected_reason, "{label}: {proof:?}");
        if expected_state == ProcessTerminalState::Observed {
            // The lease DIRECTORY is still on disk, so the session is NOT free and the projection
            // must be absent — `freeAtObservation` is a literal `true` and its presence is the
            // claim. Reaching `observed` and writing that block would be two different bugs.
            match &proof {
                ProcessTerminal::Observed {
                    canonical_session, ..
                } => assert!(
                    canonical_session.is_none(),
                    "a stale lease is not a free session: {canonical_session:?}"
                ),
                other => panic!("{other:?}"),
            }
        }
    }

    // Rung 3 of the ladder, which is the one most likely to look redundant: a dead owner whose
    // writer is still SPAWNING is never stale, because there is no writer pid to probe yet. The
    // claim stands and the run stays unproven.
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    let session_file = fx.run_dir.as_path().join("session.jsonl");
    std::fs::write(&session_file, b"{}\n").expect("write session file");
    let mut candidate = one_clean_step(&fx.run_id, &fx.instance);
    candidate.session_file = Some(session_file.clone());
    write_process_terminal_candidate(&fx.run_dir, &candidate)
        .await
        .expect("write candidate");
    let lease_dir =
        crate::background::session_lease::session_lease_dir(&session_file, &fx.lease_root)
            .expect("lease dir");
    std::fs::create_dir_all(&lease_dir).expect("create lease dir");
    std::fs::write(
        lease_dir.join("owner.json"),
        serde_json::to_vec(&owner("spawning")).expect("encode owner"),
    )
    .expect("write owner");
    assert_eq!(
        fx.finalize_with(&LeaseClaimProbe {
            hostname: HOST.to_string(),
            liveness: |_| Liveness::Dead,
            start_identity_of: |_| None::<ProcessStartIdentity>,
        })
        .await
        .reason(),
        Some(ProcessTerminalReason::CanonicalSessionLeaseActive),
        "a writer mid-fork has no pid to probe, so the lease is not reclaimable and not ignorable"
    );
}

/// `[CYRUP-DELTA]` — the two directions of a session file that cannot be resolved, which upstream
/// collapses into one.
///
/// `inspectSessionLease` opens with `realpathSync.native` and lets the throw escape into
/// `process-terminal.ts:296`'s catch, so EVERY resolution failure is `unknown` /
/// `proof-write-failed`. Here a swept transcript (`NotFound`) is an answer — no path, no key, no
/// lease directory, nothing claiming it — and the run closes `observed`, while a failure that
/// leaves the lease state genuinely unknown keeps upstream's refusal with the errno as its
/// diagnostic.
///
/// The split matters far more here than upstream, because cyrup's candidate names a session file
/// on every run rather than only on a revival: without it, a retention sweep or a cleaned worktree
/// would make every affected run's close read `unknown` for a reason that has nothing to do with
/// its processes.
#[tokio::test]
async fn finalize_over_a_swept_session_file_is_observed_and_over_an_unreadable_one_refuses() {
    // (a) the session file is simply gone.
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    let mut candidate = one_clean_step(&fx.run_id, &fx.instance);
    candidate.session_file = Some(fx.run_dir.as_path().join("swept-away.jsonl"));
    write_process_terminal_candidate(&fx.run_dir, &candidate)
        .await
        .expect("write candidate");
    let proof = fx.finalize().await;
    assert_eq!(
        proof.state(),
        ProcessTerminalState::Observed,
        "a swept transcript says nothing about this run's processes: {proof:?}"
    );
    match &proof {
        ProcessTerminal::Observed {
            canonical_session, ..
        } => assert!(canonical_session.is_none(), "{canonical_session:?}"),
        other => panic!("{other:?}"),
    }

    // (b) the path cannot be resolved for a reason that is NOT absence: a component of it is a
    // regular file, so `realpath(3)` fails with `ENOTDIR`. The lease state is genuinely unknown,
    // and upstream's refusal is kept verbatim — with the OS error as the diagnostic, which is
    // what `errorMessage(error)` carries at pi `:296`.
    let unreadable = Fixture::new();
    unreadable.write_status(1, None).await;
    let blocker = unreadable.run_dir.as_path().join("not-a-directory");
    std::fs::write(&blocker, b"i am a file\n").expect("write blocker");
    let mut candidate = one_clean_step(&unreadable.run_id, &unreadable.instance);
    candidate.session_file = Some(blocker.join("session.jsonl"));
    write_process_terminal_candidate(&unreadable.run_dir, &candidate)
        .await
        .expect("write candidate");
    let proof = unreadable.finalize().await;
    assert_eq!(
        proof.reason(),
        Some(ProcessTerminalReason::ProofWriteFailed),
        "an inspection that could not be performed is not a free session: {proof:?}"
    );
    match &proof {
        ProcessTerminal::Unknown { diagnostic, .. } => assert!(
            diagnostic.as_ref().is_some_and(|d| !d.is_empty()),
            "the refusal carries the OS error, as pi `:296` does: {proof:?}"
        ),
        other => panic!("{other:?}"),
    }
}

/// pi `:275-276`. Without this rung the runner's release → mark → finalize ordering silently stops
/// mattering, and every revived run reports `observed` whether or not its lease actually came off.
#[tokio::test]
async fn finalize_with_an_unacknowledged_revival_release_is_release_unverified() {
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    let session_file = fx.run_dir.as_path().join("session.jsonl");
    std::fs::write(&session_file, b"{}\n").expect("write session file");

    let mut candidate = one_clean_step(&fx.run_id, &fx.instance);
    candidate.session_file = Some(session_file.clone());
    candidate.revival_lease_token = Some(crate::background::session_lease::LeaseToken::from_token(
        "tok-9",
    ));
    write_process_terminal_candidate(&fx.run_dir, &candidate)
        .await
        .expect("write candidate");
    assert_eq!(
        fx.finalize().await.reason(),
        Some(ProcessTerminalReason::CanonicalSessionReleaseUnverified),
        "a held lease with no acknowledged release cannot be proven terminal"
    );

    // ...and with the acknowledgement stamped, the ladder gets past this rung and the observed
    // proof carries the session projection saying the lease WAS released.
    candidate.revival_lease_release_acknowledged = Some(true);
    write_process_terminal_candidate(&fx.run_dir, &candidate)
        .await
        .expect("rewrite candidate");
    std::fs::remove_file(fx.run_dir.process_terminal()).expect("clear the refusal");
    let proof = fx.finalize().await;
    let ProcessTerminal::Observed {
        canonical_session, ..
    } = &proof
    else {
        panic!("expected observed, got {proof:?}");
    };
    let session = canonical_session.as_ref().expect("session projection");
    assert_eq!(session.lease_disposition, LeaseDisposition::Released);
    assert!(session.free_at_observation);
    assert_eq!(session.canonical_session_lease_released, Some(true));
}

/// pi `:277`, BOTH disjuncts.
///
/// The second one is the crash-versus-clean discriminator: the candidate
/// [`initialize_process_terminal`] writes has an empty `writers` map and NO `expectedWriters`, so a
/// runner that died at startup lands here, while a runner that finished declares real counts and
/// can reach `observed`.
#[tokio::test]
async fn finalize_with_inconsistent_or_empty_writers_is_writer_close_unverified() {
    // (a) declared two children for step 0, recorded one — a child whose close was never observed.
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    write_process_terminal_candidate(
        &fx.run_dir,
        &candidate_with(
            &fx.run_id,
            &fx.instance,
            BTreeMap::from([("0".to_string(), vec![observed_writer(0)])]),
            Some(BTreeMap::from([("0".to_string(), 2)])),
        ),
    )
    .await
    .expect("write candidate");
    assert_eq!(
        fx.finalize().await.reason(),
        Some(ProcessTerminalReason::WriterCloseUnverified),
        "a launched child whose close was never seen must leave the run unproven"
    );

    // (b) the crash case: the candidate exactly as `initialize_process_terminal` leaves it.
    let crashed = Fixture::new();
    crashed.write_status(1, None).await;
    initialize_process_terminal(&crashed.run_dir, &crashed.run_id, &crashed.instance)
        .await
        .expect("initialize");
    // The launch's own `pending` sidecar must NOT short-circuit the ladder, or a crashed runner
    // would never be judged at all.
    assert_eq!(
        crashed.finalize().await.reason(),
        Some(ProcessTerminalReason::WriterCloseUnverified),
        "a runner that died before declaring anything is not a clean close"
    );

    // (c) a declared index with a non-zero count and no `writers` entry at all — upstream's OTHER
    // inconsistency direction (`:272`).
    let orphan = Fixture::new();
    orphan.write_status(1, None).await;
    write_process_terminal_candidate(
        &orphan.run_dir,
        &candidate_with(
            &orphan.run_id,
            &orphan.instance,
            BTreeMap::new(),
            Some(BTreeMap::from([("0".to_string(), 1)])),
        ),
    )
    .await
    .expect("write candidate");
    assert_eq!(
        orphan.finalize().await.reason(),
        Some(ProcessTerminalReason::WriterCloseUnverified)
    );
}

/// pi `:279`.
///
/// Vacuous upstream — `subagent-runner.ts:5168-5190` writes every `writers[i]` as `[]`, so there is
/// never a process tree to judge. LIVE here: cyrup's children are real OS processes in their own
/// groups, and a group that still holds members after the child exited is an orphaned subtree.
#[tokio::test]
async fn finalize_with_an_unobserved_writer_process_tree_is_process_tree_unverified() {
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    let unverified = ProcessInstanceExit::PiWriter {
        process_instance_id: "writer-0".to_string(),
        attempt: 0,
        close_observed_at: 1_700_000_000_000,
        exit_code: Some(0),
        signal: None,
        process_tree: ProcessTreeTerminal::Unknown {
            reason: ProcessTreeUnknownReason::VerificationFailed,
            diagnostic: Some("process group 4242 still has live members".to_string()),
        },
    };
    write_process_terminal_candidate(
        &fx.run_dir,
        &candidate_with(
            &fx.run_id,
            &fx.instance,
            BTreeMap::from([("0".to_string(), vec![unverified])]),
            Some(BTreeMap::from([("0".to_string(), 1)])),
        ),
    )
    .await
    .expect("write candidate");
    assert_eq!(
        fx.finalize().await.reason(),
        Some(ProcessTerminalReason::ProcessTreeUnverified),
        "a child whose descendants outlived it has not proven its tree terminal"
    );
}

/// pi `:282-293` — the positive proof, over the candidate a real cyrup run writes.
#[tokio::test]
async fn finalize_reaches_observed_over_real_writer_records() {
    let fx = Fixture::new();
    let session_file = fx.run_dir.as_path().join("session.jsonl");
    std::fs::write(&session_file, b"{}\n").expect("write session file");
    fx.write_status(2, Some(&session_file)).await;
    write_process_terminal_candidate(
        &fx.run_dir,
        &candidate_with(
            &fx.run_id,
            &fx.instance,
            BTreeMap::from([
                ("0".to_string(), vec![observed_writer(0)]),
                (
                    "1".to_string(),
                    vec![observed_writer(0), observed_writer(1)],
                ),
            ]),
            Some(BTreeMap::from([("0".to_string(), 1), ("1".to_string(), 2)])),
        ),
    )
    .await
    .expect("write candidate");

    let proof = fx.finalize().await;
    let ProcessTerminal::Observed {
        base,
        observed_at,
        instances,
        ..
    } = &proof
    else {
        panic!("expected observed, got {proof:?}");
    };
    assert_eq!(*observed_at, fx.close().close_observed_at);
    // pi `:290` — the runner FIRST, then every writer, flattened in step-index order.
    assert_eq!(
        instances.len(),
        4,
        "runner + three writer children: {instances:?}"
    );
    assert!(matches!(instances[0], ProcessInstanceExit::Runner { .. }));
    assert!(
        instances[1..]
            .iter()
            .all(|entry| matches!(entry, ProcessInstanceExit::PiWriter { .. }))
    );
    // A complete run whose session file is still on disk is resumable (pi `:147`).
    assert_eq!(base.resume_disposition, Some(ResumeDisposition::Resumable));

    // The event line lands exactly once, stamped with upstream's lifecycle artifact version.
    let events = fx.process_terminal_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["lifecycleArtifactVersion"], json!(3));
    assert_eq!(events[0]["runId"], json!("run-1"));
    assert_eq!(events[0]["processTerminal"]["state"], json!("observed"));

    // The overlay: the run-level proof on `status.processTerminal`, and one per step.
    let status: RunStatus =
        serde_json::from_slice(&std::fs::read(fx.run_dir.status()).expect("read status"))
            .expect("parse status");
    assert_eq!(
        status.process_terminal.as_ref().map(ProcessTerminal::state),
        Some(ProcessTerminalState::Observed)
    );
    for (index, step) in status.steps.iter().enumerate() {
        let step_proof = step.process_terminal.as_ref().expect("per-step proof");
        assert_eq!(
            step_proof.state(),
            ProcessTerminalState::Observed,
            "step {index}"
        );
        assert_eq!(step_proof.base().child_index, Some(index));
    }
}

/// pi `:248-252`. A reconciler that finalized a second time would overwrite a good proof and
/// double-emit the lifecycle event.
#[tokio::test]
async fn an_existing_observed_or_unknown_proof_short_circuits() {
    let fx = Fixture::new();
    fx.write_status(1, None).await;
    write_process_terminal_candidate(&fx.run_dir, &one_clean_step(&fx.run_id, &fx.instance))
        .await
        .expect("write candidate");

    let first = fx.finalize().await;
    assert_eq!(first.state(), ProcessTerminalState::Observed);
    let second = fx.finalize().await;
    assert_eq!(
        second, first,
        "the second finalize must return the first verbatim"
    );
    assert_eq!(
        fx.process_terminal_events().len(),
        1,
        "a second finalize must not append a second event"
    );

    // ...and an `unknown` proof short-circuits too, even though a later reader might wish it did
    // not: re-running the ladder over the same inputs cannot produce a better answer.
    let refused = Fixture::new();
    refused.write_status(1, None).await;
    let first = refused.finalize().await;
    assert_eq!(
        first.reason(),
        Some(ProcessTerminalReason::RunnerCandidateMissing)
    );
    write_process_terminal_candidate(
        &refused.run_dir,
        &one_clean_step(&refused.run_id, &refused.instance),
    )
    .await
    .expect("write candidate late");
    assert_eq!(refused.finalize().await, first);
    assert_eq!(refused.process_terminal_events().len(), 1);
}

/// pi `:299-309` — the durability ordering.
///
/// A consumer that saw the event but found no sidecar would believe a run closed with nothing able
/// to confirm it. The sidecar write comes first and NOTHING else happens without it.
#[tokio::test]
async fn a_non_durable_proof_write_emits_no_event_and_returns_unknown() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // The events file lives somewhere that exists; the RUN directory does not, so the sidecar's
    // atomic write cannot land. (A swept run directory is exactly this shape.)
    let mut events = Some(
        BoundedJsonlWriter::create(&tmp.path().join("events.jsonl"))
            .await
            .expect("open events"),
    );
    let run_dir = RunDir::for_existing(&tmp.path().join("gone"));
    let run_id = RunId::from_token("run-1");
    let instance = RunnerProcessInstanceId::from_token("instance-1");
    let proof = finalize_process_terminal(
        &run_dir,
        &run_id,
        &RunnerCloseObservation {
            process_instance_id: instance,
            close_observed_at: 1,
            exit_code: Some(0),
            signal: None,
        },
        &tmp.path().join("session-leases"),
        &mut events,
    )
    .await;

    assert_eq!(
        proof.reason(),
        Some(ProcessTerminalReason::ProofWriteFailed)
    );
    let ProcessTerminal::Unknown { diagnostic, .. } = &proof else {
        panic!("expected unknown, got {proof:?}");
    };
    assert_eq!(
        diagnostic.as_deref(),
        Some("Failed to persist process-terminal proof.")
    );
    drop(events);
    let lines = std::fs::read_to_string(tmp.path().join("events.jsonl")).unwrap_or_default();
    assert!(
        !lines.contains(PROCESS_TERMINAL_EVENT_TYPE),
        "a non-durable proof must emit no event: {lines}"
    );
    assert!(!run_dir.process_terminal().exists());
}

// -------------------------------------------------------------------------------------------
// The overlay's per-step derivation (`:233`)
// -------------------------------------------------------------------------------------------

/// pi `:233`, one dense ternary, and the place a mis-port is quietest.
///
/// Its head (`expected === 0 => "not-started"`) is what keeps a step that never launched a child
/// distinguishable from one whose writers did not add up; drop it and a skipped step reports
/// `unknown`, which reads as a defect nobody can find.
#[tokio::test]
async fn overlay_derives_each_steps_state_from_its_own_writer_counts() {
    let fx = Fixture::new();
    fx.write_status(3, None).await;
    let candidate = candidate_with(
        &fx.run_id,
        &fx.instance,
        BTreeMap::from([
            // step 0: one launched, one observed -> observed
            ("0".to_string(), vec![observed_writer(0)]),
            // step 1: two launched, one observed -> unknown
            ("1".to_string(), vec![observed_writer(0)]),
            // step 2: never dispatched -> not-started
            ("2".to_string(), Vec::new()),
        ]),
        Some(BTreeMap::from([
            ("0".to_string(), 1),
            ("1".to_string(), 2),
            ("2".to_string(), 0),
        ])),
    );
    // The RUN-level proof is observed, so the per-step derivation is the only thing that can make
    // the three steps differ.
    let proof = ProcessTerminal::Observed {
        base: ProcessTerminalBase::new(fx.run_id.clone(), fx.instance.clone()),
        observed_at: 7,
        instances: vec![ProcessInstanceExit::Runner {
            process_instance_id: fx.instance.clone(),
            close_observed_at: 7,
            exit_code: Some(0),
            signal: None,
        }],
        canonical_session: None,
    };
    overlay_status(&fx.run_dir, &proof, Some(&candidate)).await;

    let status: RunStatus =
        serde_json::from_slice(&std::fs::read(fx.run_dir.status()).expect("read status"))
            .expect("parse status");
    let states: Vec<ProcessTerminalState> = status
        .steps
        .iter()
        .map(|step| {
            step.process_terminal
                .as_ref()
                .expect("per-step proof")
                .state()
        })
        .collect();
    assert_eq!(
        states,
        vec![
            ProcessTerminalState::Observed,
            ProcessTerminalState::Unknown,
            ProcessTerminalState::NotStarted,
        ],
        "each step's state is derived from ITS OWN declared-vs-recorded counts"
    );
    // The unknown step inherits the run's reason when the run has one, and
    // `writer-close-unverified` when it does not (pi `:219`) — here the run is observed.
    assert_eq!(
        status.steps[1]
            .process_terminal
            .as_ref()
            .and_then(ProcessTerminal::reason),
        Some(ProcessTerminalReason::WriterCloseUnverified)
    );
}

/// pi `:238-240` — *"The proof sidecar remains authoritative when terminal status is
/// unavailable."* A missing or corrupt `status.json` must not invalidate a proof that just
/// persisted beside it.
#[tokio::test]
async fn overlay_swallows_an_unreadable_status() {
    let fx = Fixture::new();
    std::fs::write(fx.run_dir.status(), b"{not json").expect("write garbage status");
    let proof = ProcessTerminal::Pending {
        base: ProcessTerminalBase::new(fx.run_id.clone(), fx.instance.clone()),
    };
    overlay_status(&fx.run_dir, &proof, None).await;
    assert_eq!(
        std::fs::read_to_string(fx.run_dir.status()).expect("read status"),
        "{not json",
        "an unreadable status is left exactly as it was"
    );
}

// -------------------------------------------------------------------------------------------
// The refusal sentences — fifteen deliverables
// -------------------------------------------------------------------------------------------

/// Every sentence `process-terminal.ts` throws, byte for byte.
///
/// None of them reaches a user as an error: each is folded into an `unknown` proof's `diagnostic`
/// (`:186`, `:197`, `:297`), which is the string an operator reads out of `process-terminal.json`
/// when a run will not settle. A paraphrase makes two builds' artifacts disagree about the same
/// defect.
#[test]
fn every_refusal_sentence_is_byte_identical_to_upstream() {
    let cases: Vec<(String, &str)> = vec![
        (
            ProcessTerminalError::InvalidCandidate { dir: "/d".into() }.to_string(),
            "Invalid process-terminal candidate in '/d'.",
        ),
        (
            ProcessTerminalError::InvalidWriterRecords { index: "2".into() }.to_string(),
            "Invalid writer process records for child '2'.",
        ),
        (
            ProcessTerminalError::InvalidExpectedWriters.to_string(),
            "Invalid expected writer process records.",
        ),
        (
            ProcessTerminalError::InvalidExpectedWriterCount { index: "2".into() }.to_string(),
            "Invalid expected writer count for child '2'.",
        ),
        (
            ProcessTerminalError::InvalidCandidateSessionFile.to_string(),
            "Invalid process-terminal candidate sessionFile.",
        ),
        (
            ProcessTerminalError::InvalidCandidateLeaseToken.to_string(),
            "Invalid process-terminal candidate lease token.",
        ),
        (
            ProcessTerminalError::InvalidLeaseReleaseAcknowledgement.to_string(),
            "Invalid process-terminal lease release acknowledgement.",
        ),
        (
            ProcessTerminalError::InvalidProof { label: "/d".into() }.to_string(),
            "Invalid process-terminal proof in '/d'.",
        ),
        (
            ProcessTerminalError::ProofRunMismatch {
                label: "/d".into(),
                actual: "a".into(),
                expected: "b".into(),
            }
            .to_string(),
            "Process-terminal proof in '/d' belongs to run 'a', expected 'b'.",
        ),
        (
            ProcessTerminalError::ProofRunnerMismatch {
                label: "/d".into(),
                actual: "a".into(),
                expected: "b".into(),
            }
            .to_string(),
            "Process-terminal proof in '/d' belongs to runner 'a', expected 'b'.",
        ),
        (
            ProcessTerminalError::InvalidInstances { label: "/d".into() }.to_string(),
            "Invalid process-terminal instances in '/d'.",
        ),
        (
            ProcessTerminalError::ObservedMissingObservedAt { label: "/d".into() }.to_string(),
            "Observed process-terminal proof in '/d' is missing observedAt.",
        ),
        (
            ProcessTerminalError::ObservedMissingInstances { label: "/d".into() }.to_string(),
            "Observed process-terminal proof in '/d' is missing instances.",
        ),
        (
            ProcessTerminalError::ObservedMissingRunnerInstance { label: "/d".into() }.to_string(),
            "Observed process-terminal proof in '/d' has no matching runner instance.",
        ),
        (
            ProcessTerminalError::InvalidResumeDisposition { label: "/d".into() }.to_string(),
            "Invalid process-terminal resume disposition in '/d'.",
        ),
    ];
    assert_eq!(cases.len(), 15, "all fifteen upstream throws are covered");
    for (actual, expected) in cases {
        assert_eq!(actual, expected);
    }
}

/// pi `validateProof` (`:161-178`) — each guard reached through the value that trips it, so the
/// sentences above are not merely spelled but actually raised.
#[test]
fn validate_proof_raises_each_guard_from_a_value_that_trips_it() {
    let run_id = RunId::from_token("run-1");
    let instance = RunnerProcessInstanceId::from_token("inst-1");
    let expectation = ProofExpectation::new(&run_id, &instance);
    let base = json!({
        "version": 1, "state": "pending", "runId": "run-1", "runnerProcessInstanceId": "inst-1",
    });

    assert!(validate_proof(&base, "/d", expectation).is_ok());
    // `:162` — a state word this build does not know.
    assert_eq!(
        validate_proof(
            &json!({ "version": 1, "state": "wedged", "runId": "run-1",
                                "runnerProcessInstanceId": "inst-1" }),
            "/d",
            expectation
        )
        .unwrap_err()
        .to_string(),
        "Invalid process-terminal proof in '/d'."
    );
    // `:165` / `:166` — the two identity checks.
    assert_eq!(
        validate_proof(
            &json!({ "version": 1, "state": "pending", "runId": "other",
                                "runnerProcessInstanceId": "inst-1" }),
            "/d",
            expectation
        )
        .unwrap_err()
        .to_string(),
        "Process-terminal proof in '/d' belongs to run 'other', expected 'run-1'."
    );
    assert_eq!(
        validate_proof(
            &json!({ "version": 1, "state": "pending", "runId": "run-1",
                                "runnerProcessInstanceId": "other" }),
            "/d",
            expectation
        )
        .unwrap_err()
        .to_string(),
        "Process-terminal proof in '/d' belongs to runner 'other', expected 'inst-1'."
    );
    // `:167-169`.
    let mut bad_instances = base.clone();
    bad_instances["instances"] = json!([{ "kind": "runner" }]);
    assert_eq!(
        validate_proof(&bad_instances, "/d", expectation)
            .unwrap_err()
            .to_string(),
        "Invalid process-terminal instances in '/d'."
    );
    // `:171` / `:172` / `:174` — the observed arm's three additional debts.
    let mut observed = base.clone();
    observed["state"] = json!("observed");
    assert_eq!(
        validate_proof(&observed, "/d", expectation)
            .unwrap_err()
            .to_string(),
        "Observed process-terminal proof in '/d' is missing observedAt."
    );
    observed["observedAt"] = json!(1);
    assert_eq!(
        validate_proof(&observed, "/d", expectation)
            .unwrap_err()
            .to_string(),
        "Observed process-terminal proof in '/d' is missing instances."
    );
    observed["instances"] = json!([]);
    assert_eq!(
        validate_proof(&observed, "/d", expectation)
            .unwrap_err()
            .to_string(),
        "Observed process-terminal proof in '/d' has no matching runner instance."
    );
    // A runner instance that is present but belongs to a DIFFERENT runner is the same refusal.
    observed["instances"] = json!([{
        "processInstanceId": "someone-else", "kind": "runner",
        "closeObservedAt": 1, "exitCode": 0, "signal": null,
    }]);
    assert_eq!(
        validate_proof(&observed, "/d", expectation)
            .unwrap_err()
            .to_string(),
        "Observed process-terminal proof in '/d' has no matching runner instance."
    );
    observed["instances"] = json!([{
        "processInstanceId": "inst-1", "kind": "runner",
        "closeObservedAt": 1, "exitCode": 0, "signal": null,
    }]);
    assert!(validate_proof(&observed, "/d", expectation).is_ok());
    // `:176`.
    let mut bad_resume = base;
    bad_resume["resumeDisposition"] = json!("maybe");
    assert_eq!(
        validate_proof(&bad_resume, "/d", expectation)
            .unwrap_err()
            .to_string(),
        "Invalid process-terminal resume disposition in '/d'."
    );
}

/// pi `validProcessInstance` (`:37-57`) — including the two ABSENCE rules a decode-then-check port
/// silently drops, and the `windows-taskkill` asymmetry that is upstream's, not this port's.
#[test]
fn valid_process_instance_reproduces_upstreams_exact_accept_set() {
    let writer = json!({
        "processInstanceId": "w", "kind": "pi-writer", "attempt": 0,
        "closeObservedAt": 1, "exitCode": 0, "signal": null,
        "processTree": { "state": "observed", "mechanism": "posix-process-group",
                         "processGroupId": 7, "verifiedAt": 2 },
    });
    assert!(valid_process_instance(
        &writer,
        Some(InstanceKind::PiWriter)
    ));
    assert!(valid_process_instance(&writer, None));
    assert!(!valid_process_instance(&writer, Some(InstanceKind::Runner)));

    // `:44` — a runner record that carries an `attempt` is REFUSED.
    let runner = json!({
        "processInstanceId": "r", "kind": "runner",
        "closeObservedAt": 1, "exitCode": null, "signal": "SIGKILL",
    });
    assert!(valid_process_instance(&runner, Some(InstanceKind::Runner)));
    let mut runner_with_attempt = runner.clone();
    runner_with_attempt["attempt"] = json!(0);
    assert!(
        !valid_process_instance(&runner_with_attempt, Some(InstanceKind::Runner)),
        "a runner exit has no attempt ordinal"
    );

    // `:50` — a non-positive process group id.
    let mut zero_group = writer.clone();
    zero_group["processTree"]["processGroupId"] = json!(0);
    assert!(!valid_process_instance(
        &zero_group,
        Some(InstanceKind::PiWriter)
    ));

    // [CYRUP-DELTA] `:46-53` accepts ONLY `posix-process-group` on the observed side, so an
    // observed `windows-taskkill` tree — a shape upstream's own `types.ts:660-665` declares — is
    // refused. Reproduced deliberately; see `valid_process_instance`'s own doc.
    let mut taskkill = writer.clone();
    taskkill["processTree"] =
        json!({ "state": "observed", "mechanism": "windows-taskkill", "pid": 9, "verifiedAt": 2 });
    assert!(!valid_process_instance(
        &taskkill,
        Some(InstanceKind::PiWriter)
    ));

    // `:54-56` — the three unknown reasons, and nothing else.
    for reason in [
        "unsupported-platform",
        "signal-failed",
        "verification-failed",
    ] {
        let mut unknown = writer.clone();
        unknown["processTree"] = json!({ "state": "unknown", "reason": reason });
        assert!(
            valid_process_instance(&unknown, Some(InstanceKind::PiWriter)),
            "{reason}"
        );
    }
    let mut bogus = writer;
    bogus["processTree"] = json!({ "state": "unknown", "reason": "because" });
    assert!(!valid_process_instance(
        &bogus,
        Some(InstanceKind::PiWriter)
    ));
}

/// pi `sanitizeProcessTerminal` (`:180-188`). One corrupt `status.processTerminal` key must
/// degrade, never fail: `debug.run` and the async-status projection read it on every call.
#[test]
fn sanitize_turns_a_bad_overlay_into_proof_write_failed_and_never_panics() {
    let value = json!({ "version": 1, "state": "wedged" });
    let sanitized = sanitize_process_terminal(Some(&value), ProofExpectation::none(), "status")
        .expect("a present value always yields a proof");
    assert_eq!(
        sanitized.reason(),
        Some(ProcessTerminalReason::ProofWriteFailed)
    );
    assert_eq!(sanitized.run_id().as_str(), "status");
    assert_eq!(sanitized.runner_process_instance_id().as_str(), "unknown");
    let ProcessTerminal::Unknown { diagnostic, .. } = &sanitized else {
        panic!("expected unknown");
    };
    assert_eq!(
        diagnostic.as_deref(),
        Some("Invalid process-terminal proof in 'status'.")
    );

    assert!(sanitize_process_terminal(None, ProofExpectation::none(), "status").is_none());
    assert!(
        sanitize_process_terminal(
            Some(&serde_json::Value::Null),
            ProofExpectation::none(),
            "s"
        )
        .is_none()
    );
}

/// The same degradation, reached through `RunStatus`'s own decode — which is where it actually
/// matters. A `#[derive(Deserialize)]` on the typed field would make ONE corrupt key fail every
/// status read, every listing and every control verb.
#[test]
fn a_corrupt_status_overlay_degrades_instead_of_failing_the_whole_status_read() {
    let mut raw = serde_json::to_value(RunStatus::queued(
        RunId::from_token("run-1"),
        RunMode::Single,
        Some(1),
    ))
    .expect("encode status");
    raw["processTerminal"] = json!({ "version": 1, "state": "wedged" });
    let status: RunStatus = serde_json::from_value(raw).expect("the status still parses");
    assert_eq!(
        status
            .process_terminal
            .as_ref()
            .and_then(ProcessTerminal::reason),
        Some(ProcessTerminalReason::ProofWriteFailed)
    );
}

// -------------------------------------------------------------------------------------------
// The candidate's own validator, and the lease-release stamp
// -------------------------------------------------------------------------------------------

/// pi `readProcessTerminalCandidate` (`:75-112`). A missing file is `None`; every malformed shape
/// is its own sentence.
#[tokio::test]
async fn reading_a_candidate_refuses_each_malformed_shape() {
    let fx = Fixture::new();
    assert!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .expect("a missing candidate is not an error")
            .is_none()
    );

    let write = |value: serde_json::Value| {
        std::fs::write(
            fx.run_dir.process_terminal_candidate(),
            serde_json::to_vec(&value).expect("encode"),
        )
        .expect("write candidate");
    };
    let good = json!({
        "version": 1, "runId": "run-1", "runnerProcessInstanceId": "inst-1", "writers": {},
    });

    write(good.clone());
    assert!(read_process_terminal_candidate(&fx.run_dir).await.is_ok());

    let mut bad = good.clone();
    bad["version"] = json!(2);
    write(bad);
    assert!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .unwrap_err()
            .to_string()
            .starts_with("Invalid process-terminal candidate in ")
    );

    let mut bad = good.clone();
    bad["writers"] = json!({ "0": [{ "kind": "pi-writer" }] });
    write(bad);
    assert_eq!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .unwrap_err()
            .to_string(),
        "Invalid writer process records for child '0'."
    );

    let mut bad = good.clone();
    bad["expectedWriters"] = json!([]);
    write(bad);
    assert_eq!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .unwrap_err()
            .to_string(),
        "Invalid expected writer process records."
    );

    let mut bad = good.clone();
    bad["expectedWriters"] = json!({ "0": -1 });
    write(bad);
    assert_eq!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .unwrap_err()
            .to_string(),
        "Invalid expected writer count for child '0'."
    );

    for (key, sentence) in [
        (
            "sessionFile",
            "Invalid process-terminal candidate sessionFile.",
        ),
        (
            "revivalLeaseToken",
            "Invalid process-terminal candidate lease token.",
        ),
    ] {
        let mut bad = good.clone();
        bad[key] = json!(7);
        write(bad);
        assert_eq!(
            read_process_terminal_candidate(&fx.run_dir)
                .await
                .unwrap_err()
                .to_string(),
            sentence
        );
    }

    let mut bad = good;
    bad["revivalLeaseReleaseAcknowledged"] = json!("yes");
    write(bad);
    assert_eq!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .unwrap_err()
            .to_string(),
        "Invalid process-terminal lease release acknowledgement."
    );
}

/// pi `markProcessTerminalCandidateLeaseRelease` (`:134-138`). The acknowledgement is about ONE
/// lease: stamping it against a successor's token would clear the
/// `canonical-session-release-unverified` rung for a lease that was never released.
#[tokio::test]
async fn the_lease_release_stamp_only_lands_on_the_matching_token() {
    use crate::background::session_lease::LeaseToken;
    let fx = Fixture::new();
    let mut candidate = one_clean_step(&fx.run_id, &fx.instance);
    candidate.revival_lease_token = Some(LeaseToken::from_token("tok-mine"));
    write_process_terminal_candidate(&fx.run_dir, &candidate)
        .await
        .expect("write candidate");

    mark_process_terminal_candidate_lease_release(
        &fx.run_dir,
        &LeaseToken::from_token("tok-someone-else"),
        true,
    )
    .await;
    assert_eq!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .expect("reads")
            .expect("exists")
            .revival_lease_release_acknowledged,
        None,
        "a foreign token must not stamp this candidate"
    );

    mark_process_terminal_candidate_lease_release(
        &fx.run_dir,
        &LeaseToken::from_token("tok-mine"),
        true,
    )
    .await;
    assert_eq!(
        read_process_terminal_candidate(&fx.run_dir)
            .await
            .expect("reads")
            .expect("exists")
            .revival_lease_release_acknowledged,
        Some(true)
    );
}

/// pi `resumeDisposition` (`:144-148`). A STOPPED run is deliberately non-resumable; a terminal run
/// whose transcript was swept is `unavailable` however terminal it looks.
#[test]
fn resume_disposition_reads_the_state_word_and_the_file_on_disk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let present = tmp.path().join("session.jsonl");
    std::fs::write(&present, b"{}\n").expect("write session");
    let missing = tmp.path().join("gone.jsonl");

    assert_eq!(
        resume_disposition(Some("stopped"), Some(&present)),
        ResumeDisposition::NonResumable
    );
    for terminal in ["complete", "completed", "failed", "paused"] {
        assert_eq!(
            resume_disposition(Some(terminal), Some(&present)),
            ResumeDisposition::Resumable,
            "{terminal}"
        );
        assert_eq!(
            resume_disposition(Some(terminal), Some(&missing)),
            ResumeDisposition::Unavailable,
            "{terminal} with no transcript on disk"
        );
        assert_eq!(
            resume_disposition(Some(terminal), None),
            ResumeDisposition::Unavailable,
            "{terminal} with no transcript at all"
        );
    }
    assert_eq!(
        resume_disposition(Some("running"), Some(&present)),
        ResumeDisposition::Unavailable
    );
    assert_eq!(
        resume_disposition(None, Some(&present)),
        ResumeDisposition::Unavailable
    );
}

/// The wire spellings `resume_disposition` and the overlay both key on, pinned against the derive
/// that actually writes them — the one place a rename would otherwise go unnoticed.
#[test]
fn the_state_wire_words_match_what_serde_writes() {
    for state in [
        RunState::Queued,
        RunState::Running,
        RunState::Paused,
        RunState::Complete,
        RunState::Failed,
        RunState::Stopped,
    ] {
        assert_eq!(
            serde_json::to_value(state).expect("encode"),
            json!(state.as_wire_word())
        );
    }
    for state in [
        crate::background::StepState::Pending,
        crate::background::StepState::Running,
        crate::background::StepState::Paused,
        crate::background::StepState::Complete,
        crate::background::StepState::Failed,
        crate::background::StepState::Stopped,
    ] {
        assert_eq!(
            serde_json::to_value(state).expect("encode"),
            json!(state.as_wire_word())
        );
    }
}

/// The candidate and the proof are on-disk formats shared with upstream: camelCase keys, the
/// discriminated unions flattened exactly as `types.ts` declares them.
#[test]
fn the_on_disk_shapes_match_upstreams_json() {
    let candidate = ProcessTerminalCandidate {
        version: ProcessTerminalVersion,
        run_id: RunId::from_token("run-1"),
        runner_process_instance_id: RunnerProcessInstanceId::from_token("inst-1"),
        writers: BTreeMap::from([("0".to_string(), vec![observed_writer(1)])]),
        expected_writers: Some(BTreeMap::from([("0".to_string(), 1)])),
        session_file: Some(PathBuf::from("/tmp/s.jsonl")),
        revival_lease_token: Some(crate::background::session_lease::LeaseToken::from_token(
            "tok-1",
        )),
        revival_lease_release_acknowledged: Some(true),
    };
    assert_eq!(
        serde_json::to_value(&candidate).expect("encode"),
        json!({
            "version": 1,
            "runId": "run-1",
            "runnerProcessInstanceId": "inst-1",
            "writers": { "0": [{
                "kind": "pi-writer",
                "processInstanceId": "writer-1",
                "attempt": 1,
                "closeObservedAt": 1_700_000_000_000_i64,
                "exitCode": 0,
                "signal": null,
                "processTree": {
                    "state": "observed",
                    "mechanism": "posix-process-group",
                    "processGroupId": 4242,
                    "verifiedAt": 1_700_000_000_001_i64,
                },
            }] },
            "expectedWriters": { "0": 1 },
            "sessionFile": "/tmp/s.jsonl",
            "revivalLeaseToken": "tok-1",
            "revivalLeaseReleaseAcknowledged": true,
        })
    );
    let decoded: ProcessTerminalCandidate =
        serde_json::from_value(serde_json::to_value(&candidate).expect("encode"))
            .expect("round-trips");
    assert_eq!(decoded, candidate);

    let unknown = unknown_proof(
        RunId::from_token("run-1"),
        RunnerProcessInstanceId::from_token("inst-1"),
        ProcessTerminalReason::ProcessTreeUnverified,
        Some("why".to_string()),
    );
    assert_eq!(
        serde_json::to_value(&unknown).expect("encode"),
        json!({
            "version": 1,
            "state": "unknown",
            "runId": "run-1",
            "runnerProcessInstanceId": "inst-1",
            "reason": "process-tree-unverified",
            "diagnostic": "why",
        })
    );
    // pi `:141` — an EMPTY diagnostic is omitted, never written as `""`.
    let bare = unknown_proof(
        RunId::from_token("run-1"),
        RunnerProcessInstanceId::from_token("inst-1"),
        ProcessTerminalReason::StaleRepair,
        Some(String::new()),
    );
    assert_eq!(
        serde_json::to_value(&bare).expect("encode")["diagnostic"],
        serde_json::Value::Null
    );
}

//! SCOPE_9's behaviour tests — the pool, the verdict ladder, and the two admission entry points.
//!
//! The §D1/§D2/§D3 findings each have a named guard here: §D1 is pinned in
//! [`super::config`]'s own tests (the resolver returns an out-of-range value VERBATIM),
//! §D2 by [`a_slot_is_released_when_the_run_reaches_terminal`] (there is no `release()` to call),
//! and §D3's no-proof fallback by [`a_completed_run_with_a_dead_runner_pid_releases_its_slot`]
//! (the proof rung above it has its own guards) — the one test a
//! verbatim upstream port fails.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::background::atomic::write_atomic_json;
use crate::background::reconcile::Liveness;
use crate::background::{RunDir, RunId, RunMode, RunState, RunStatus, StepStatus};
use crate::error::SubagentError;
use crate::identity::{IndexSegment, SessionId};
use crate::registration::AbandonedSlotRelease;

// -------------------------------------------------------------------------------------------
// Fixtures
// -------------------------------------------------------------------------------------------

/// A genuinely reaped pid — the `background/reconcile.rs` idiom, so "dead" means the kernel says
/// `ESRCH` rather than a stubbed probe agreeing with itself.
fn reaped_pid() -> u32 {
    let mut child = std::process::Command::new("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("`true` spawns");
    let pid = child.id();
    let _ = child.wait();
    pid
}

fn session(id: &str) -> SessionId {
    SessionId::parse(id).expect("non-empty")
}

/// Options against a temp capacity root, with the DEFAULT abandoned policy and the REAL liveness
/// probe — every test that needs otherwise says so in its own body.
fn options(root: &Path) -> CapacityOptions {
    CapacityOptions::new(root.join("capacity")).with_abandoned_slot_release(
        AbandonedSlotRelease::After(DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS),
    )
}

fn at(now: i64, options: CapacityOptions) -> CapacityOptions {
    options.with_now(Arc::new(move || now))
}

fn probing(liveness: Liveness, options: CapacityOptions) -> CapacityOptions {
    options.with_pid_liveness(Arc::new(move |_| liveness))
}

/// A run directory under `<root>/async/<run id>` with a `status.json` in it.
async fn write_run(
    root: &Path,
    run_id: &RunId,
    session_id: &SessionId,
    state: RunState,
    pid: Option<u32>,
) -> PathBuf {
    let async_dir = root.join("async").join(run_id.as_str());
    tokio::fs::create_dir_all(&async_dir).await.expect("mkdir");
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, pid);
    status.state = state;
    status.session_id = Some(session_id.clone());
    status.pid = pid;
    status.last_update = 0;
    write_atomic_json(&RunDir::for_existing(&async_dir).status(), &status)
        .await
        .expect("status write");
    async_dir
}

/// Replace a run's `status.json` wholesale — for the fields `write_run` does not take.
async fn patch_status(async_dir: &Path, mutate: impl FnOnce(&mut RunStatus)) {
    let path = RunDir::for_existing(async_dir).status();
    let bytes = tokio::fs::read(&path).await.expect("read status");
    let mut status: RunStatus = serde_json::from_slice(&bytes).expect("parse status");
    mutate(&mut status);
    write_atomic_json(&path, &status)
        .await
        .expect("status write");
}

/// Write one owner record straight into `<pool>/slot-<n>`, bypassing [`acquire`].
///
/// Several rungs of the verdict ladder describe states no admission can produce on purpose — a
/// started reservation whose durable pid bind never landed, a corrupt `owner.json`, a record filed
/// under the wrong key. Seeding them directly is how those rungs get tested at all.
async fn seed_slot(options: &CapacityOptions, owner: &ActiveAsyncCapacityOwner) -> PathBuf {
    let dir = slot_dir(
        &session_pool_dir(options.root_dir(), &owner.owner_session_id),
        owner.slot,
    );
    tokio::fs::create_dir_all(&dir).await.expect("mkdir");
    write_atomic_json(&dir.join("owner.json"), owner)
        .await
        .expect("owner write");
    dir
}

fn owner_record(
    session_id: &SessionId,
    run_id: &RunId,
    async_dir: &Path,
    kind: ActiveAsyncCapacityKind,
) -> ActiveAsyncCapacityOwner {
    ActiveAsyncCapacityOwner {
        version: CapacityOwnerVersion,
        reservation_token: "token-1".to_string(),
        owner_session_id: session_id.clone(),
        owner_session_key: IndexSegment::encode(session_id.as_str()).to_string(),
        slot: 0,
        run_id: run_id.clone(),
        source_run_id: None,
        generation: 0,
        kind,
        async_dir: async_dir.to_path_buf(),
        reserved_at: 0,
        runner_process_instance_id: None,
        runner_pid: None,
        runner_process_start_identity: None,
        runner_started_at: None,
    }
}

async fn pool_slot_count(options: &CapacityOptions, session_id: &SessionId) -> usize {
    key::occupied_slots(&session_pool_dir(options.root_dir(), session_id))
        .await
        .expect("listing")
        .len()
}

// -------------------------------------------------------------------------------------------
// 1, 19 — the core property and the refusal
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn two_sessions_each_get_their_own_cap() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let (a, b) = (session("session-a"), session("session-b"));
    let run_a1 = RunId::from_token("a1");
    let run_a2 = RunId::from_token("a2");
    let run_b1 = RunId::from_token("b1");
    let dir_a1 = write_run(tmp.path(), &run_a1, &a, RunState::Running, Some(1)).await;
    let dir_a2 = write_run(tmp.path(), &run_a2, &a, RunState::Running, Some(2)).await;
    let dir_b1 = write_run(tmp.path(), &run_b1, &b, RunState::Running, Some(3)).await;

    let first = acquire(
        AcquireInput {
            session_id: &a,
            limit: Some(1),
            run_id: &run_a1,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir_a1,
        },
        &options,
    )
    .await
    .expect("first admission")
    .expect("a slot");
    assert_eq!(first.owner().slot, 0);

    // Session A is at its cap.
    let refused = acquire(
        AcquireInput {
            session_id: &a,
            limit: Some(1),
            run_id: &run_a2,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir_a2,
        },
        &options,
    )
    .await
    .expect_err("A is full");
    assert!(matches!(
        refused,
        SubagentError::ActiveAsyncCapacityExhausted(_)
    ));
    assert_eq!(
        refused.to_string(),
        "Active async run capacity exhausted: 1/1 used.",
        "pi `active-async-capacity.ts:74` verbatim, trailing period included"
    );

    // THE CORE PROPERTY: session B is unaffected.
    assert!(
        acquire(
            AcquireInput {
                session_id: &b,
                limit: Some(1),
                run_id: &run_b1,
                kind: ActiveAsyncCapacityKind::Runner,
                async_dir: &dir_b1,
            },
            &options,
        )
        .await
        .expect("B admits")
        .is_some()
    );

    let pool_a = session_pool_dir(options.root_dir(), &a);
    let pool_b = session_pool_dir(options.root_dir(), &b);
    assert!(pool_a.is_dir() && pool_b.is_dir());
    assert_ne!(pool_a, pool_b);
    assert_eq!(pool_a.parent(), pool_b.parent(), "siblings under one root");
}

#[tokio::test]
async fn the_refusal_message_is_upstreams_verbatim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    for index in 0..2u32 {
        let run_id = RunId::from_token(format!("r{index}"));
        let dir = write_run(tmp.path(), &run_id, &s, RunState::Running, Some(index + 1)).await;
        acquire(
            AcquireInput {
                session_id: &s,
                limit: Some(2),
                run_id: &run_id,
                kind: ActiveAsyncCapacityKind::Runner,
                async_dir: &dir,
            },
            &options,
        )
        .await
        .expect("admitted")
        .expect("a slot");
    }
    let run_id = RunId::from_token("r2");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Running, Some(9)).await;
    let refused = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(2),
            run_id: &run_id,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir,
        },
        &options,
    )
    .await
    .expect_err("full");
    assert_eq!(
        refused.to_string(),
        "Active async run capacity exhausted: 2/2 used."
    );
}

// -------------------------------------------------------------------------------------------
// 2, 11, 12, 13 — the verdict ladder
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_slot_is_released_when_the_run_reaches_terminal() {
    // §D2: there is no `release()` to call. The slot comes back because the NEXT acquire
    // reconciles first — asserting a `release()` exists is the failure mode this test prevents.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let first = RunId::from_token("first");
    let dir = write_run(tmp.path(), &first, &s, RunState::Running, None).await;
    let pid = reaped_pid();

    let mut handle = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &first,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");
    handle
        .mark_started(
            pid,
            crate::background::process_terminal::RunnerProcessInstanceId::new(),
        )
        .await
        .expect("bind");
    assert_eq!(handle.owner().runner_pid, Some(pid));

    // Still running: the slot is held.
    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &options)
            .await
            .expect("reconcile")
            .used,
        1
    );

    patch_status(&dir, |status| {
        status.state = RunState::Complete;
        status.pid = Some(pid);
    })
    .await;

    // The NEXT admission in the same session succeeds, and the pool still holds exactly one slot.
    let second = RunId::from_token("second");
    let second_dir = write_run(tmp.path(), &second, &s, RunState::Running, None).await;
    let handle = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &second,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &second_dir,
        },
        &options,
    )
    .await
    .expect("the terminal run's slot came back")
    .expect("a slot");
    assert_eq!(handle.owner().run_id, second);
    assert_eq!(
        snapshot_for(&s, Some(1), &options)
            .await
            .expect("snapshot")
            .used,
        1
    );
}

#[tokio::test]
async fn a_completed_run_with_a_dead_runner_pid_releases_its_slot() {
    // §D3's regression guard, and it covers the NO-PROOF path specifically: this run wrote no
    // `process-terminal.json` at all, which is exactly what a runner killed before its own close
    // leaves behind. Without the fallback ladder beneath the proof rung the verdict is `retained`
    // FOREVER — a `Complete` run with no proof is not `failed`, so the abandoned-timeout ladder
    // never takes it either — and after `limit` such runs the session could never spawn again.
    // The proof rung's own positive case is pinned separately, in
    // `a_matching_observed_proof_releases_the_slot_before_the_pid_is_ever_probed`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let run_id = RunId::from_token("done");
    let pid = reaped_pid();
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(pid)).await;

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_pid = Some(pid);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(verdict.is_releasable(), "got {verdict:?}");
    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &options)
            .await
            .expect("reconcile")
            .used,
        0
    );
}

#[tokio::test]
async fn an_unknown_pid_liveness_never_releases_a_slot() {
    // `background/reconcile.rs`'s standing rule: an `EPERM`-class probe failure is not evidence of
    // death. Reclaiming a LIVE run's slot is the failure mode the whole cap exists to avoid.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Unknown, at(10_000_000, options(tmp.path())));
    let s = session("s");
    let run_id = RunId::from_token("maybe-alive");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Failed, Some(4242)).await;
    patch_status(&dir, |status| {
        status.telemetry.last_activity_at = Some(0);
    })
    .await;

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_pid = Some(4242);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    match &verdict {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert!(reason.contains("unknown"), "got {reason}");
        }
        other => panic!("an unknown liveness must never release: {other:?}"),
    }
    assert_eq!(pool_slot_count(&options, &s).await, 1);
}

#[tokio::test]
async fn a_paused_run_keeps_its_slot() {
    // `RunState::is_terminal` excludes `Paused` and so does pi's `terminalState` (`:212-214`): a
    // paused run is resumable and its runner may still be alive.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Dead, options(tmp.path()));
    let s = session("s");
    let run_id = RunId::from_token("paused");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Paused, Some(7)).await;
    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_pid = Some(7);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    match owner_release_verdict(&owner, &options).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert_eq!(reason, "run is still paused");
        }
        other => panic!("a paused run must keep its slot: {other:?}"),
    }
}

#[tokio::test]
async fn a_reservation_with_no_runner_identity_is_retained() {
    // pi `:218` — a slot claimed but never bound is reconciliation's to leave alone; only
    // `rollback` may take it back.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Dead, options(tmp.path()));
    let s = session("s");
    let run_id = RunId::from_token("unbound");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(7)).await;
    let owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    seed_slot(&options, &owner).await;

    match owner_release_verdict(&owner, &options).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert_eq!(reason, "runner process identity has not been recorded");
        }
        other => panic!("{other:?}"),
    }
}

// -------------------------------------------------------------------------------------------
// 3, 7 — the abandoned-timeout ladder
// -------------------------------------------------------------------------------------------

/// The one shape that reaches the abandoned ladder: a reservation whose `runnerStartedAt` landed
/// but whose durable pid bind did not (pi names this window at `:392-393`). It is `is_started`, so
/// it passes the identity rung; it has no owner pid, so the no-proof FALLBACK ladder cannot fire
/// either; and the RUN's own `status.pid` is what the abandoned ladder probes — pi's own split
/// between `owner.runnerProcessInstanceId` (`:230`) and `status.pid` (`:244`).
async fn seed_abandoned(
    tmp: &Path,
    options: &CapacityOptions,
    s: &SessionId,
    last_activity_at: i64,
) -> (ActiveAsyncCapacityOwner, PathBuf) {
    let run_id = RunId::from_token("abandoned");
    let pid = reaped_pid();
    let dir = write_run(tmp, &run_id, s, RunState::Failed, Some(pid)).await;
    patch_status(&dir, |status| {
        status.telemetry.last_activity_at = Some(last_activity_at);
    })
    .await;
    let mut owner = owner_record(s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_started_at = Some(last_activity_at);
    seed_slot(options, &owner).await;
    (owner, dir)
}

#[tokio::test]
async fn an_abandoned_slot_releases_after_the_configured_delay() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let now = 10 * DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS;
    let options = at(now, options(tmp.path()));
    let s = session("s");
    let (owner, dir) = seed_abandoned(
        tmp.path(),
        &options,
        &s,
        now - DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS - 1,
    )
    .await;

    let verdict = owner_release_verdict(&owner, &options).await;
    match &verdict {
        ActiveAsyncCapacityReleaseVerdict::Releasable {
            evidence: Some(evidence),
            reason,
        } => {
            assert_eq!(evidence.released_by, "abandoned-timeout");
            assert_eq!(evidence.process_proof, "unknown");
            assert_eq!(evidence.runner_pid, "gone");
            assert_eq!(
                evidence.abandoned_slot_release_after_ms,
                DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS
            );
            assert!(reason.starts_with("abandoned-timeout:"), "got {reason}");
        }
        other => panic!("{other:?}"),
    }

    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &options)
            .await
            .expect("reconcile")
            .used,
        0
    );
    // pi `appendAbandonedReleaseEvent` (`:196-210`) — the diagnostic lands beside the run whose
    // slot was taken.
    let events = tokio::fs::read_to_string(RunDir::for_existing(&dir).events())
        .await
        .expect("events.jsonl");
    assert!(
        events.contains("\"type\":\"subagent.capacity.released\""),
        "{events}"
    );
    assert!(
        events.contains("\"releasedBy\":\"abandoned-timeout\""),
        "{events}"
    );
    assert!(events.contains("\"processProof\":\"unknown\""), "{events}");
    assert!(events.contains("\"runnerPid\":\"gone\""), "{events}");
}

#[tokio::test]
async fn an_abandoned_slot_does_not_release_before_the_configured_delay() {
    // pi `:250` uses `<=`, so EXACTLY the threshold is retained and so is anything under it. BOTH
    // ages are exercised: a `<` port passes the one-millisecond-under case and fails the boundary,
    // which is the only case that tells the two spellings apart.
    let now = 10 * DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS;
    for offset in [1, 0] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let options = at(now, options(tmp.path()));
        let s = session("s");
        let (owner, _) = seed_abandoned(
            tmp.path(),
            &options,
            &s,
            now - DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS + offset,
        )
        .await;
        let age = DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS - offset;
        match owner_release_verdict(&owner, &options).await {
            ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
                assert_eq!(
                    reason,
                    format!(
                        // pi `:239` prefixes EVERY retained reason on this ladder with the
                        // proof state it fell through — `process-terminal proof is
                        // ${proofState}`. cyrup's own pid rung appends to that prefix rather than
                        // replacing it, so an operator reading the sentence can tell "there was no
                        // proof" from "the proof said something this rung did not accept".
                        "process-terminal proof is missing; runner pid was never recorded; last \
                         activity age {age}ms has not exceeded abandoned-timeout \
                         {DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS}ms"
                    ),
                );
            }
            other => panic!("age {age}ms must be retained: {other:?}"),
        }
        // And the sweep agrees with the verdict — the slot is still charged to the session.
        assert_eq!(
            reconcile_active_async_capacity(&s, Some(1), &options)
                .await
                .expect("reconcile")
                .used,
            1,
        );
        assert_eq!(pool_slot_count(&options, &s).await, 1);
    }
}

#[tokio::test]
async fn release_after_false_means_never_release() {
    // All three states in ONE test, because the bug this guards is the two-state collapse.
    let tmp = tempfile::tempdir().expect("tempdir");
    let now = 10 * DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS;
    let aged_out = now - DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS - 1;

    // `Some(Never)` — the literal JSON `false`.
    let never = at(now, options(tmp.path())).with_abandoned_slot_release(
        resolve_abandoned_slot_release(Some(AbandonedSlotRelease::Never)),
    );
    let s_never = session("never");
    let (owner, _) = seed_abandoned(tmp.path(), &never, &s_never, aged_out).await;
    match owner_release_verdict(&owner, &never).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert!(
                reason.contains("abandoned-timeout policy is disabled"),
                "got {reason}"
            );
        }
        other => panic!("`false` must never release: {other:?}"),
    }

    // `None` — unset, so the DEFAULT ladder applies and the same slot IS released.
    let unset_tmp = tempfile::tempdir().expect("tempdir");
    let unset = at(now, options(unset_tmp.path()))
        .with_abandoned_slot_release(resolve_abandoned_slot_release(None));
    let s_unset = session("unset");
    let (owner, _) = seed_abandoned(unset_tmp.path(), &unset, &s_unset, aged_out).await;
    assert!(owner_release_verdict(&owner, &unset).await.is_releasable());

    // `Some(After(n))` — an explicit threshold, honoured verbatim.
    let explicit_tmp = tempfile::tempdir().expect("tempdir");
    let explicit = at(now, options(explicit_tmp.path())).with_abandoned_slot_release(
        resolve_abandoned_slot_release(Some(AbandonedSlotRelease::After(
            MIN_ABANDONED_SLOT_RELEASE_AFTER_MS,
        ))),
    );
    let s_explicit = session("explicit");
    let (owner, _) = seed_abandoned(
        explicit_tmp.path(),
        &explicit,
        &s_explicit,
        now - MIN_ABANDONED_SLOT_RELEASE_AFTER_MS - 1,
    )
    .await;
    match owner_release_verdict(&owner, &explicit).await {
        ActiveAsyncCapacityReleaseVerdict::Releasable {
            evidence: Some(evidence),
            ..
        } => assert_eq!(
            evidence.abandoned_slot_release_after_ms,
            MIN_ABANDONED_SLOT_RELEASE_AFTER_MS
        ),
        other => panic!("{other:?}"),
    }
}

// -------------------------------------------------------------------------------------------
// 4 + §1.1a(b) — the workflow arm
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_live_workflows_slot_is_never_reclaimed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let s = session("s");
    let run_id = RunId::from_token("wf");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(5)).await;
    patch_status(&dir, |status| status.mode = RunMode::Workflow).await;

    let live: HashSet<RunId> = std::iter::once(run_id.clone()).collect();
    let held = options(tmp.path()).with_live_workflow_run_ids(live);
    let owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Workflow);
    seed_slot(&held, &owner).await;

    match owner_release_verdict(&owner, &held).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert_eq!(reason, "workflow controller is still live");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(pool_slot_count(&held, &s).await, 1);

    // The second half is what proves the set is actually consulted: drop the id and the very same
    // slot becomes releasable.
    let gone = options(tmp.path());
    assert!(owner_release_verdict(&owner, &gone).await.is_releasable());
    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &gone)
            .await
            .expect("reconcile")
            .used,
        0
    );
}

#[tokio::test]
async fn a_workflow_child_that_never_started_but_recorded_an_error_does_not_pin_its_parents_slot() {
    // §1.1a(b) — pi's `v0.68.0` not-started guard. Without it the child's missing runner identity
    // retains the parent's slot forever.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Dead, options(tmp.path()));
    let s = session("s");
    let parent = RunId::from_token("wf");
    let child = RunId::from_token("child");

    let child_dir = write_run(tmp.path(), &child, &s, RunState::Failed, None).await;
    patch_status(&child_dir, |status| {
        status.pid = None;
        status.error = Some("failed before child startup completed".to_string());
    })
    .await;

    let dir = write_run(tmp.path(), &parent, &s, RunState::Complete, Some(5)).await;
    patch_status(&dir, |status| {
        status.mode = RunMode::Workflow;
        let mut step = StepStatus::pending("agent-a");
        step.run_id = Some(child.clone());
        status.steps = vec![step];
    })
    .await;

    let owner = owner_record(&s, &parent, &dir, ActiveAsyncCapacityKind::Workflow);
    seed_slot(&options, &owner).await;
    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(verdict.is_releasable(), "got {verdict:?}");

    // Without the recorded error the same child DOES pin the parent — the guard is keyed on the
    // error, not merely on the missing pid.
    patch_status(&child_dir, |status| status.error = None).await;
    match owner_release_verdict(&owner, &options).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert!(
                reason.contains("has no runner process identity"),
                "got {reason}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_workflow_child_that_is_still_running_pins_its_parents_slot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Dead, options(tmp.path()));
    let s = session("s");
    let parent = RunId::from_token("wf");
    let child = RunId::from_token("child");
    write_run(tmp.path(), &child, &s, RunState::Running, Some(11)).await;

    let dir = write_run(tmp.path(), &parent, &s, RunState::Complete, Some(5)).await;
    patch_status(&dir, |status| {
        status.mode = RunMode::Workflow;
        let mut step = StepStatus::pending("agent-a");
        step.run_id = Some(child.clone());
        status.steps = vec![step];
    })
    .await;
    let owner = owner_record(&s, &parent, &dir, ActiveAsyncCapacityKind::Workflow);
    seed_slot(&options, &owner).await;

    match owner_release_verdict(&owner, &options).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert!(reason.contains("is still running"), "got {reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_runner_status_on_a_workflow_slot_is_retained() {
    // pi `:269` — the owner says "workflow" and the status says otherwise; nothing here is safe to
    // conclude, so the slot stays.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let run_id = RunId::from_token("wf");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(5)).await;
    let owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Workflow);
    match owner_release_verdict(&owner, &options).await {
        ActiveAsyncCapacityReleaseVerdict::Retained { reason } => {
            assert_eq!(reason, "status mode is single, not workflow");
        }
        other => panic!("{other:?}"),
    }
}

// -------------------------------------------------------------------------------------------
// 8, 9, 10 — the limit and the key
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn no_configured_cap_means_unlimited() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    for configured in [None, Some(0)] {
        assert_eq!(resolve_max_active_async_runs_per_session(configured), None);
        for index in 0..10u32 {
            let run_id = RunId::from_token(format!("r{index}"));
            let dir = write_run(tmp.path(), &run_id, &s, RunState::Running, Some(1)).await;
            assert!(
                acquire(
                    AcquireInput {
                        session_id: &s,
                        limit: resolve_max_active_async_runs_per_session(configured),
                        run_id: &run_id,
                        kind: ActiveAsyncCapacityKind::Runner,
                        async_dir: &dir,
                    },
                    &options,
                )
                .await
                .expect("unlimited never refuses")
                .is_none(),
                "an unconfigured cap writes no slot at all"
            );
        }
    }
    assert!(
        !options.root_dir().exists(),
        "an unconfigured install must touch the capacity tree ZERO times"
    );
}

#[tokio::test]
async fn the_capacity_root_is_keyed_by_session_not_cwd() {
    // One session, two working directories, ONE `Roots` — the pool address is identical, and the
    // second spawn from the other cwd sees the first spawn's slot.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("one-session");
    let cwd_a = tmp.path().join("project-a");
    let cwd_b = tmp.path().join("project-b");
    // No assertion that the pool address is cwd-independent: `session_pool_dir` takes no cwd, so
    // the property is guaranteed by the signature and any equality check here would compare an
    // expression with itself. What follows is the BEHAVIOURAL proof, which is what matters — the
    // second acquire runs from a different cwd and is refused by the first one's slot.

    let run_a = RunId::from_token("ra");
    let dir_a = write_run(&cwd_a, &run_a, &s, RunState::Running, Some(1)).await;
    acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &run_a,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir_a,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");

    let run_b = RunId::from_token("rb");
    let dir_b = write_run(&cwd_b, &run_b, &s, RunState::Running, Some(2)).await;
    let refused = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &run_b,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir_b,
        },
        &options,
    )
    .await
    .expect_err("the other cwd shares the SAME pool");
    assert!(matches!(
        refused,
        SubagentError::ActiveAsyncCapacityExhausted(_)
    ));
    assert_eq!(pool_slot_count(&options, &s).await, 1);
}

#[test]
fn a_session_id_that_is_a_path_produces_one_component() {
    let root = Path::new("/capacity");
    let s = session("/home/u/.cyrup/sessions/x.jsonl");
    let pool = session_pool_dir(root, &s);
    assert_eq!(pool.parent(), Some(root), "depth 1 — no traversal");
    let leaf = pool.file_name().and_then(|n| n.to_str()).expect("a leaf");
    // `is_portable_segment` rejects a trailing extension, so this id takes the hash branch — the
    // on-disk-format claim.
    assert!(leaf.starts_with("~sha256-"), "got {leaf}");
    // Pinned against an INDEPENDENTLY computed digest, never against `IndexSegment::encode`:
    // `session_pool_dir` is defined as `root.join(encode(id))`, so comparing the two compares an
    // expression with itself and would pass just as happily for a key that kept the separators.
    // `hashed_segment` hashes the PRE-encoding value, so this is
    // `~sha256-` + sha256("/home/u/.cyrup/sessions/x.jsonl").
    assert_eq!(
        leaf,
        "~sha256-7cfc27445678db1d08b232a529cafbdd5b974ba48e363b5f4f85727872b1277b"
    );
    assert!(
        !leaf.contains(std::path::MAIN_SEPARATOR),
        "a session id that IS a path must not become one: {leaf}"
    );
}

// -------------------------------------------------------------------------------------------
// 14, 15, 16 — on-disk robustness and concurrency
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_slot_whose_owner_session_key_disagrees_with_its_directory_is_skipped_not_deleted() {
    // pi `:344-348` — every guard `continue`s. Deleting the record would hand the slot to another
    // admission while whatever wrote it may still be running.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Dead, options(tmp.path()));
    let s = session("s");
    let run_id = RunId::from_token("misfiled");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(1)).await;

    // (a) the record's own key disagrees with its session id — refused by the parser.
    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_pid = Some(1);
    owner.owner_session_key = IndexSegment::encode("someone-else").to_string();
    let slot = seed_slot(&options, &owner).await;
    assert!(read_owner(&slot).await.is_none(), "unparseable");
    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &options)
            .await
            .expect("reconcile")
            .used,
        1,
        "skipped, and still counted"
    );
    assert!(slot.is_dir(), "never deleted");
    tokio::fs::remove_dir_all(&slot).await.expect("cleanup");

    // (b) the record parses but sits in a directory whose name disagrees with its `slot` field —
    // the address guard, which cannot live in the parser.
    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_pid = Some(1);
    owner.runner_started_at = Some(1);
    owner.slot = 3;
    let misaddressed = session_pool_dir(options.root_dir(), &s).join("slot-0");
    tokio::fs::create_dir_all(&misaddressed)
        .await
        .expect("mkdir");
    write_atomic_json(&misaddressed.join("owner.json"), &owner)
        .await
        .expect("owner write");
    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &options)
            .await
            .expect("reconcile")
            .used,
        1
    );
    assert!(misaddressed.is_dir(), "never deleted");
}

#[tokio::test]
async fn a_corrupt_owner_json_keeps_the_slot_occupied() {
    // `createSlot`'s fail-closed comment (`:376-377`): an unparseable owner is a slot that stays
    // OCCUPIED rather than becoming available to another admission.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let slot = session_pool_dir(options.root_dir(), &s).join("slot-0");
    tokio::fs::create_dir_all(&slot).await.expect("mkdir");
    tokio::fs::write(slot.join("owner.json"), b"{ not json")
        .await
        .expect("write");

    assert_eq!(
        reconcile_active_async_capacity(&s, Some(1), &options)
            .await
            .expect("reconcile")
            .used,
        1
    );
    let run_id = RunId::from_token("new");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Running, Some(1)).await;
    assert!(matches!(
        acquire(
            AcquireInput {
                session_id: &s,
                limit: Some(1),
                run_id: &run_id,
                kind: ActiveAsyncCapacityKind::Runner,
                async_dir: &dir,
            },
            &options,
        )
        .await
        .expect_err("fail closed"),
        SubagentError::ActiveAsyncCapacityExhausted(_)
    ));
    assert!(slot.join("owner.json").is_file(), "never deleted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_acquires_in_one_session_never_share_a_slot() {
    const N: u32 = 8;
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let mut dirs = Vec::new();
    for index in 0..N {
        let run_id = RunId::from_token(format!("r{index}"));
        dirs.push((
            run_id.clone(),
            write_run(tmp.path(), &run_id, &s, RunState::Running, Some(index + 1)).await,
        ));
    }

    let mut tasks = Vec::new();
    for (run_id, dir) in dirs {
        let options = options.clone();
        let s = s.clone();
        tasks.push(tokio::spawn(async move {
            acquire(
                AcquireInput {
                    session_id: &s,
                    limit: Some(N),
                    run_id: &run_id,
                    kind: ActiveAsyncCapacityKind::Runner,
                    async_dir: &dir,
                },
                &options,
            )
            .await
            .map(|handle| {
                handle.map(|handle| {
                    (
                        handle.owner().slot,
                        handle.owner().reservation_token.clone(),
                    )
                })
            })
        }));
    }

    let mut slots = HashSet::new();
    let mut tokens = HashSet::new();
    for task in tasks {
        let (slot, token) = task
            .await
            .expect("join")
            .expect("every admission fits")
            .expect("a slot");
        assert!(slots.insert(slot), "slot {slot} was handed out twice");
        assert!(tokens.insert(token), "a reservation token was reused");
    }
    assert_eq!(slots.len(), N as usize);
    assert_eq!(pool_slot_count(&options, &s).await, N as usize);
}

// -------------------------------------------------------------------------------------------
// rollback, transfer, inspect
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn rollback_returns_an_unstarted_slot_and_refuses_a_started_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let run_id = RunId::from_token("r");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Running, None).await;

    let mut handle = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &run_id,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");
    assert!(
        handle.rollback().await,
        "an unstarted reservation is undone"
    );
    assert_eq!(pool_slot_count(&options, &s).await, 0);

    let mut handle = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &run_id,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");
    handle
        .mark_started(
            4242,
            crate::background::process_terminal::RunnerProcessInstanceId::new(),
        )
        .await
        .expect("bind");
    assert!(
        !handle.rollback().await,
        "a started run's slot is reconciliation's, never a rollback's"
    );
    assert_eq!(pool_slot_count(&options, &s).await, 1);
}

#[tokio::test]
async fn a_transfer_moves_one_slot_and_bumps_its_generation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let source = RunId::from_token("source");
    let resumed = RunId::from_token("resumed");
    let source_dir = write_run(tmp.path(), &source, &s, RunState::Running, Some(1)).await;
    let resumed_dir = write_run(tmp.path(), &resumed, &s, RunState::Running, Some(2)).await;

    let mut handle = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &source,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &source_dir,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");
    handle
        .mark_started(
            4242,
            crate::background::process_terminal::RunnerProcessInstanceId::new(),
        )
        .await
        .expect("bind");

    // pi `:495-496` — a still-running source is a HARD error, never a second admission.
    let refused = transfer(
        TransferInput {
            session_id: &s,
            limit: Some(1),
            source_run_id: &source,
            run_id: &resumed,
            async_dir: &resumed_dir,
        },
        &options,
    )
    .await
    .expect_err("not transferable while running");
    assert_eq!(
        refused.to_string(),
        "Active async capacity source 'source' is not transferable."
    );
    assert_eq!(pool_slot_count(&options, &s).await, 1);

    patch_status(&source_dir, |status| status.state = RunState::Complete).await;
    let moved = transfer(
        TransferInput {
            session_id: &s,
            limit: Some(1),
            source_run_id: &source,
            run_id: &resumed,
            async_dir: &resumed_dir,
        },
        &options,
    )
    .await
    .expect("transferable")
    .expect("a handle");
    assert_eq!(moved.owner().run_id, resumed);
    assert_eq!(moved.owner().source_run_id.as_ref(), Some(&source));
    assert_eq!(moved.owner().generation, 1);
    assert_eq!(
        moved.owner().runner_pid,
        None,
        "the new run has not started"
    );
    assert_eq!(
        pool_slot_count(&options, &s).await,
        1,
        "one slot, not two — that is the whole point"
    );

    // The breadcrumb makes the source's inspection meaningful rather than "no slot records this".
    let inspection = inspect_active_async_capacity_owner(&source, Some(&s), None, &options)
        .await
        .expect("inspect");
    assert_eq!(inspection.relation, CapacityRelation::Source);
    assert_eq!(
        inspection.release,
        ActiveAsyncCapacityReleaseVerdict::NotOwned {
            reason: "slot was transferred to resumed".to_string()
        }
    );
}

#[tokio::test]
async fn a_transfer_with_no_source_slot_is_an_ordinary_admission() {
    // pi `:513`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let source = RunId::from_token("never-had-one");
    let resumed = RunId::from_token("resumed");
    let dir = write_run(tmp.path(), &resumed, &s, RunState::Running, Some(2)).await;
    let handle = transfer(
        TransferInput {
            session_id: &s,
            limit: Some(1),
            source_run_id: &source,
            run_id: &resumed,
            async_dir: &dir,
        },
        &options,
    )
    .await
    .expect("falls through to acquire")
    .expect("a slot");
    assert_eq!(handle.owner().generation, 0);
    assert_eq!(handle.owner().source_run_id, None);
}

#[tokio::test]
async fn inspect_finds_a_slot_by_run_id_or_by_async_dir_and_reports_none_otherwise() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let run_id = RunId::from_token("r");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Running, Some(1)).await;
    acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(1),
            run_id: &run_id,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &dir,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");

    let by_id = inspect_active_async_capacity_owner(&run_id, Some(&s), None, &options)
        .await
        .expect("inspect");
    assert_eq!(by_id.relation, CapacityRelation::Current);
    assert_eq!(by_id.owner.as_ref().map(|o| o.slot), Some(0));

    // With no session named, every pool under the root is scanned (pi `capacitySessionDirs`).
    let by_dir = inspect_active_async_capacity_owner(
        &RunId::from_token("other"),
        None,
        Some(&dir),
        &options,
    )
    .await
    .expect("inspect");
    assert_eq!(by_dir.relation, CapacityRelation::Current);

    let missing =
        inspect_active_async_capacity_owner(&RunId::from_token("nope"), Some(&s), None, &options)
            .await
            .expect("inspect");
    assert_eq!(missing.relation, CapacityRelation::None);
    assert!(missing.owner.is_none());
    assert_eq!(
        missing.release,
        ActiveAsyncCapacityReleaseVerdict::NotOwned {
            reason: "no active-capacity slot records this run".to_string()
        }
    );
}

#[tokio::test]
async fn a_contended_transfer_claim_is_an_error_and_never_a_second_admission() {
    // pi `:511` — a slot whose `capacity.claim` is already held means a concurrent transfer is in
    // flight. Falling through to `acquire` there would charge the session TWICE for what the
    // operator sees as one run, which is the whole failure `transfer` exists to avoid.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let source = RunId::from_token("source");
    let resumed = RunId::from_token("resumed");
    let source_dir = write_run(tmp.path(), &source, &s, RunState::Complete, Some(1)).await;
    let resumed_dir = write_run(tmp.path(), &resumed, &s, RunState::Running, Some(2)).await;

    let owner = owner_record(&s, &source, &source_dir, ActiveAsyncCapacityKind::Runner);
    let slot = seed_slot(&options, &owner).await;
    // Another process holds the O_EXCL lockfile (`claim.rs`'s `CLAIM_FILE`).
    tokio::fs::write(slot.join("capacity.claim"), b"another-process")
        .await
        .expect("claim");

    let refused = transfer(
        TransferInput {
            session_id: &s,
            limit: Some(2),
            source_run_id: &source,
            run_id: &resumed,
            async_dir: &resumed_dir,
        },
        &options,
    )
    .await
    .expect_err("the claim is held");
    assert_eq!(
        refused.to_string(),
        "Active async capacity transfer is already in progress for run 'source'."
    );
    assert_eq!(
        pool_slot_count(&options, &s).await,
        1,
        "no second slot was charged"
    );
    // The contended claim belongs to the other process and must survive untouched.
    assert_eq!(
        tokio::fs::read(slot.join("capacity.claim"))
            .await
            .expect("claim survives"),
        b"another-process",
    );
    assert_eq!(
        read_owner(&slot).await.expect("owner").run_id,
        source,
        "the slot still records the source run"
    );
}

#[tokio::test]
async fn mark_started_refuses_once_the_slot_has_moved_on() {
    // pi `:396` — `matchingOwner` compares token AND run id AND generation, and a transfer bumps
    // exactly the generation. A stale handle must not be able to stamp its pid onto a slot that
    // now belongs to another run; if it could, the new run's reservation would read as started and
    // become unrollbackable.
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path());
    let s = session("s");
    let first = RunId::from_token("first");
    let resumed = RunId::from_token("resumed");
    let first_dir = write_run(tmp.path(), &first, &s, RunState::Complete, Some(1)).await;
    let resumed_dir = write_run(tmp.path(), &resumed, &s, RunState::Running, Some(2)).await;

    let mut stale = acquire(
        AcquireInput {
            session_id: &s,
            limit: Some(2),
            run_id: &first,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: &first_dir,
        },
        &options,
    )
    .await
    .expect("admitted")
    .expect("a slot");

    transfer(
        TransferInput {
            session_id: &s,
            limit: Some(2),
            source_run_id: &first,
            run_id: &resumed,
            async_dir: &resumed_dir,
        },
        &options,
    )
    .await
    .expect("transferable")
    .expect("a handle");

    let error = stale
        .mark_started(
            4242,
            crate::background::process_terminal::RunnerProcessInstanceId::new(),
        )
        .await
        .expect_err("ownership changed");
    assert_eq!(
        error.to_string(),
        "Active async capacity ownership changed for run 'first'."
    );

    let slot = slot_dir(&session_pool_dir(options.root_dir(), &s), 0);
    let owner = read_owner(&slot).await.expect("owner");
    assert_eq!(owner.run_id, resumed, "the slot is the transferee's");
    assert_eq!(owner.generation, 1);
    assert_eq!(
        owner.runner_pid, None,
        "the stale handle's pid never landed on the new owner"
    );
    assert_eq!(pool_slot_count(&options, &s).await, 1);
}

// =================================================================================================
// The proof rung — pi `active-async-capacity.ts:222-235`, now real
// =================================================================================================

/// DoD 6, first half — the process-terminal proof is the FIRST rung, and it is consulted before
/// the pid is ever probed.
///
/// The injected liveness probe PANICS. A verdict that reached it at all fails the test, so this
/// cannot pass by accident on a run whose pid happens to be gone.
///
/// Gutted (the proof rung moved below the pid ladder, or dropped): the probe panics. Gutted the
/// other way — the match on run id / instance id removed — is covered by the second half below,
/// where a proof belonging to ANOTHER runner must NOT release this slot.
#[tokio::test]
async fn a_matching_observed_proof_releases_the_slot_before_the_pid_is_ever_probed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path()).with_pid_liveness(Arc::new(|_| {
        panic!("the pid ladder must not be reached when a matching observed proof exists")
    }));
    let s = session("s");
    let run_id = RunId::from_token("proven");
    // A live pid, so every fallback rung would RETAIN: only the proof can release this.
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(1)).await;

    let instance = crate::background::process_terminal::RunnerProcessInstanceId::new();
    seed_observed_proof(&dir, &run_id, &instance).await;

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_process_instance_id = Some(instance);
    owner.runner_pid = Some(1);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(verdict.is_releasable(), "got {verdict:?}");
    assert_eq!(
        verdict.reason(),
        "matching observed process-terminal proof is present",
        "pi `:233`'s own sentence"
    );
}

/// The other half of the same rung: a proof written by a DIFFERENT runner in the same directory
/// must not release this slot.
///
/// This is what the whole `RunnerProcessInstanceId` mint exists to make meaningful. Gutted (the
/// identity match dropped), a proof left behind by a previous run in a reused directory releases
/// a live run's slot, and the cap starts admitting over its limit.
#[tokio::test]
async fn a_proof_from_another_runner_instance_does_not_release_the_slot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Alive, options(tmp.path()));
    let s = session("s");
    let run_id = RunId::from_token("foreign-proof");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(1)).await;

    // The proof on disk belongs to a runner instance this slot never bound.
    let stranger = crate::background::process_terminal::RunnerProcessInstanceId::new();
    seed_observed_proof(&dir, &run_id, &stranger).await;

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_process_instance_id =
        Some(crate::background::process_terminal::RunnerProcessInstanceId::new());
    owner.runner_pid = Some(1);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(!verdict.is_releasable(), "got {verdict:?}");
    // `read_process_terminal` runs the sidecar through `validate_proof` with this reader's own
    // expectation, so a foreign proof degrades to `unknown` rather than being believed.
    assert!(
        verdict
            .reason()
            .starts_with("process-terminal proof is unknown;"),
        "got {}",
        verdict.reason()
    );
}

/// P12 — pi's early-failure carve-out (`:222-226`). The `[CYRUP-DELTA]` that called this rung
/// "unrepresentable and dropped" is now FALSE and has been deleted.
///
/// A run that failed before its child ever started has no close to observe, so no proof will ever
/// be written for it. Without this rung its slot waits out the entire abandoned timeout for no
/// information gained — and with a live pid, never releases at all.
///
/// Gutted: the probe below panics, exactly as in the proof-rung test, because the carve-out sits
/// above the pid ladder too.
#[tokio::test]
async fn a_pre_startup_failure_releases_on_the_not_started_carve_out() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = options(tmp.path()).with_pid_liveness(Arc::new(|_| {
        panic!("the pid ladder must not be reached for a run that never started")
    }));
    let s = session("s");
    let run_id = RunId::from_token("never-started");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Failed, Some(1)).await;

    let instance = crate::background::process_terminal::RunnerProcessInstanceId::new();
    let not_started = crate::background::process_terminal::ProcessTerminal::NotStarted {
        base: crate::background::process_terminal::ProcessTerminalBase::new(
            run_id.clone(),
            instance.clone(),
        ),
    };
    patch_status(&dir, |status| {
        status.process_terminal = Some(not_started);
        status.error = Some("spawn failed before the child started".to_string());
    })
    .await;

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_process_instance_id = Some(instance);
    owner.runner_pid = Some(1);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(verdict.is_releasable(), "got {verdict:?}");
    assert_eq!(
        verdict.reason(),
        "run failed before child startup completed"
    );
}

/// The carve-out's own counter-rung (pi `:225`): a `not-started` proof with NO error does not
/// release. An empty error string does not either — pi tests `typeof … === "string" && status
/// .error`, so the empty string is falsy there too.
///
/// Gutted (the error test dropped), a run still sitting in `not-started` because its runner has
/// not yet published anything releases its slot to a second admission while it is alive.
#[tokio::test]
async fn a_not_started_proof_without_an_error_does_not_release() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let options = probing(Liveness::Alive, options(tmp.path()));
    let s = session("s");
    let run_id = RunId::from_token("not-started-clean");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Failed, Some(1)).await;

    let instance = crate::background::process_terminal::RunnerProcessInstanceId::new();
    let not_started = crate::background::process_terminal::ProcessTerminal::NotStarted {
        base: crate::background::process_terminal::ProcessTerminalBase::new(
            run_id.clone(),
            instance.clone(),
        ),
    };
    patch_status(&dir, |status| {
        status.process_terminal = Some(not_started.clone());
        status.error = Some(String::new());
    })
    .await;

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_process_instance_id = Some(instance);
    owner.runner_pid = Some(1);
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(
        !verdict.is_releasable(),
        "an empty error is falsy: {verdict:?}"
    );
}

/// DoD 8, at the capacity seam — the fallback ladder probes with
/// [`check_pid_identity_with`](crate::background::reconcile::check_pid_identity_with), not bare liveness.
///
/// The runner's pid is ALIVE, so `kill(pid, 0)` alone says "keep the slot" forever. The owner
/// recorded the identity that pid held at the bind, and the identity it holds NOW differs — the
/// number was recycled. That is `PARITY-GAPS.md:1451`'s named defect, and this is the test that
/// pins it closed at the place the ledger row names.
///
/// Gutted back to `options.pid_liveness(pid)`: the recycled pid reads `Alive`, the verdict falls
/// to the abandoned-timeout ladder, and a `Complete` run is never `Failed` — so the slot is
/// retained for the life of the machine.
#[tokio::test]
async fn the_fallback_ladder_releases_a_recycled_runner_pid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let s = session("s");
    let run_id = RunId::from_token("recycled");
    let dir = write_run(tmp.path(), &run_id, &s, RunState::Complete, Some(4242)).await;

    let options = options(tmp.path())
        .with_pid_liveness(Arc::new(|_| Liveness::Alive))
        .with_pid_start_identity(Arc::new(|_| {
            Some(crate::background::session_lease::ProcessStartIdentity::from_token("linux:999"))
        }));

    let mut owner = owner_record(&s, &run_id, &dir, ActiveAsyncCapacityKind::Runner);
    owner.runner_process_instance_id =
        Some(crate::background::process_terminal::RunnerProcessInstanceId::new());
    owner.runner_pid = Some(4242);
    owner.runner_process_start_identity =
        Some(crate::background::session_lease::ProcessStartIdentity::from_token("linux:100"));
    owner.runner_started_at = Some(1);
    seed_slot(&options, &owner).await;

    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(verdict.is_releasable(), "got {verdict:?}");
    assert_eq!(
        verdict.reason(),
        "process-terminal proof is missing; runner pid 4242 is confirmed gone and the run is \
         terminal"
    );

    // The counter-rung: the SAME live pid under the identity the owner recorded is NOT gone, and
    // the slot is retained. An over-eager rung here reclaims a live run's slot.
    let options = options.with_pid_start_identity(Arc::new(|_| {
        Some(crate::background::session_lease::ProcessStartIdentity::from_token("linux:100"))
    }));
    let verdict = owner_release_verdict(&owner, &options).await;
    assert!(!verdict.is_releasable(), "got {verdict:?}");
}

/// Write an `observed` process-terminal sidecar for `run_id`/`instance`, in the shape
/// `finalize_process_terminal` writes one.
async fn seed_observed_proof(
    async_dir: &Path,
    run_id: &RunId,
    instance: &crate::background::process_terminal::RunnerProcessInstanceId,
) {
    let proof = crate::background::process_terminal::ProcessTerminal::Observed {
        base: crate::background::process_terminal::ProcessTerminalBase::new(
            run_id.clone(),
            instance.clone(),
        ),
        observed_at: 1_700_000_000_000,
        instances: vec![
            crate::background::process_terminal::ProcessInstanceExit::Runner {
                process_instance_id: instance.clone(),
                close_observed_at: 1_700_000_000_000,
                exit_code: Some(0),
                signal: None,
            },
        ],
        canonical_session: None,
    };
    write_atomic_json(&RunDir::for_existing(async_dir).process_terminal(), &proof)
        .await
        .expect("proof write");
}

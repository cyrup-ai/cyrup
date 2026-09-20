//! The lease's behaviour tests — the four staleness rungs, the rename-claim, the per-owner
//! tombstone, the two conflict sentences, and the strict owner validator.
//!
//! **Every test here confines its root.** `Roots::sandboxed(tmp.path())` resolves
//! `session_leases_root_in` under a per-test tempdir, and every lease entry point takes its
//! `root_dir` explicitly with NO default, so nothing in this file can reach the machine-wide root
//! by omission. `paths.rs` records why that matters: an unconfined root once left 59,321 files
//! behind, and a leaked lease directory under a shared root refuses every later revival of that
//! session file on the machine.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;

/// pi's `instanceof SessionLeaseConflictError` (`session-lease.ts:58-64`), spelled once for this
/// module. It lives HERE and not on [`SessionLeaseError`] because production never asks the
/// question — see that type's own note.
fn is_conflict(error: &SessionLeaseError) -> bool {
    matches!(
        error,
        SessionLeaseError::Conflict(_) | SessionLeaseError::ConflictUnreadableOwner { .. }
    )
}
use serde_json::json;
use std::path::Path;

fn owner_json(overrides: serde_json::Value) -> serde_json::Value {
    let mut base = json!({
        "version": 1,
        "token": "tok-1",
        "canonicalSessionFile": "/tmp/session.jsonl",
        "runId": "run-new",
        "sourceRunId": "run-old",
        "pid": 4242,
        "hostname": "here",
        "writerState": "none",
        "acquiredAt": "2026-09-20T00:00:00.000Z",
        "acquiredAtMs": 1_700_000_000_000_i64,
        "updatedAtMs": 1_700_000_000_000_i64,
    });
    let (Some(object), Some(extra)) = (base.as_object_mut(), overrides.as_object()) else {
        panic!("both fixtures are objects");
    };
    for (key, value) in extra {
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    base
}

/// pi `parseOwner` (`:129-131`, `:141-142`). Each refusal is a permanent deadlock if dropped:
/// a `running` owner with no `writerPid` can never satisfy `demonstrablyStale`'s rung 4
/// (`:179-180`), so the lease is never reclaimable and every future revival of that session
/// file is refused forever — from ONE malformed byte.
#[test]
fn parse_owner_refuses_the_two_cross_field_shapes_and_a_non_positive_pid() {
    assert!(
        parse_owner(&owner_json(json!({}))).is_some(),
        "the baseline owner must parse"
    );

    // `:141` — `running` requires `writerPid`.
    assert!(parse_owner(&owner_json(json!({"writerState": "running"}))).is_none());
    // ... and with it, it parses, carrying the pid BY TYPE.
    let running = parse_owner(&owner_json(
        json!({"writerState": "running", "writerPid": 77}),
    ))
    .expect("running + writerPid parses");
    assert_eq!(
        running.writer,
        LeaseWriter::Running {
            pid: 77,
            start_identity: None
        }
    );

    // `:142` — a non-`running` state may carry neither writer field.
    assert!(parse_owner(&owner_json(json!({"writerState": "none", "writerPid": 7}))).is_none());
    assert!(
        parse_owner(&owner_json(
            json!({"writerState": "spawning", "writerProcessStartIdentity": "linux:1"})
        ))
        .is_none()
    );

    // `:129-131` — the pid must be a positive integer, because `kill(0, 0)` addresses the
    // whole process group and `kill(-n, 0)` addresses a group.
    assert!(parse_owner(&owner_json(json!({"pid": 0}))).is_none());
    assert!(parse_owner(&owner_json(json!({"pid": -1}))).is_none());
    assert!(parse_owner(&owner_json(json!({"pid": 1.5}))).is_none());

    // `:124` — a record from a build this one does not understand.
    assert!(parse_owner(&owner_json(json!({"version": 2}))).is_none());
    // `:133` — an unknown writer state word.
    assert!(parse_owner(&owner_json(json!({"writerState": "wedged"}))).is_none());
    // `:134-136` — a missing required field.
    assert!(parse_owner(&owner_json(json!({"hostname": null}))).is_none());
}

/// pi `canonicalSessionId` (`:100-104`) hashes the REALPATH. A revival addressed through a
/// symlink and one addressed through the target must contend for ONE lease directory;
/// hashing the raw argument instead would let both run and both write the session file.
#[test]
fn canonical_session_id_is_the_sha256_of_the_realpath() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("real-session.jsonl");
    std::fs::write(&target, b"{}\n").expect("write target");
    let link = tmp.path().join("aliased-session.jsonl");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");

    let direct = canonical_session_id(&target).expect("digest of the target");
    let through_link = canonical_session_id(&link).expect("digest through the symlink");
    assert_eq!(direct, through_link, "one session file, one lease key");
    assert_eq!(direct.as_str().len(), 64, "sha256 hex");
    assert!(direct.as_str().chars().all(|c| c.is_ascii_hexdigit()));

    let root = tmp.path().join("leases");
    assert_eq!(
        session_lease_dir(&target, &root).expect("lease dir"),
        session_lease_dir(&link, &root).expect("lease dir through the symlink"),
    );
    assert_eq!(
        canonical_session_file_path(&link).expect("realpath"),
        std::fs::canonicalize(&target).expect("realpath of the target"),
    );
}

/// pi `inspectSessionLease` (`:110-119`) — the three states the process-terminal ladder maps
/// onto three distinct proof outcomes. A directory that exists with an unreadable owner must
/// NOT read as free: upstream refuses to reclaim it "without proof that the owner is stale".
#[test]
fn inspect_reports_free_owned_and_unreadable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session = tmp.path().join("session.jsonl");
    std::fs::write(&session, b"{}\n").expect("write session");
    let root = tmp.path().join("leases");

    let free = inspect_session_lease(&session, &root).expect("inspect");
    assert!(free.is_free(), "no lease directory is free: {free:?}");

    let lease_dir = session_lease_dir(&session, &root).expect("lease dir");
    std::fs::create_dir_all(&lease_dir).expect("create lease dir");
    let unreadable = inspect_session_lease(&session, &root).expect("inspect");
    assert!(!unreadable.is_free());
    assert!(matches!(unreadable, SessionLeaseState::Unreadable { .. }));

    std::fs::write(
        lease_dir.join("owner.json"),
        serde_json::to_vec(&owner_json(json!({"parentSessionId": "sess-1"})))
            .expect("encode owner"),
    )
    .expect("write owner");
    let owned = inspect_session_lease(&session, &root).expect("inspect");
    assert!(!owned.is_free());
    let SessionLeaseState::Owned { owner, .. } = owned else {
        panic!("a parsable owner.json is `owned`");
    };
    assert_eq!(owner.run_id, "run-new");
    assert_eq!(owner.source_run_id, "run-old");
    assert_eq!(owner.parent_session_id.as_deref(), Some("sess-1"));
    assert_eq!(owner.pid, 4242);
    assert_eq!(owner.hostname, "here");
    assert_eq!(owner.token.as_str(), "tok-1");
    assert_eq!(owner.writer.state_word(), "none");
    assert_eq!(owner.acquired_at_ms, 1_700_000_000_000);
    assert_eq!(owner.updated_at_ms, 1_700_000_000_000);
    assert_eq!(owner.acquired_at, "2026-09-20T00:00:00.000Z");
    assert_eq!(
        owner.canonical_session_file,
        Path::new("/tmp/session.jsonl")
    );
    assert!(owner.process_start_identity.is_none());

    // A lease directory whose owner.json is garbage is `unreadable`, never `free`.
    std::fs::write(lease_dir.join("owner.json"), b"{not json").expect("clobber owner");
    assert!(matches!(
        inspect_session_lease(&session, &root).expect("inspect"),
        SessionLeaseState::Unreadable { .. }
    ));
}

/// The start identity is a bare string on the wire (pi `processStartIdentity?: string`), and
/// EQUALITY is the only operation performed on it.
#[test]
fn a_process_start_identity_round_trips_as_a_bare_string() {
    let parsed =
        parse_owner(&owner_json(json!({"processStartIdentity": "linux:900"}))).expect("parses");
    assert_eq!(
        parsed.process_start_identity,
        Some(ProcessStartIdentity::from_token("linux:900"))
    );
    assert_eq!(
        serde_json::to_string(&ProcessStartIdentity::from_token("linux:900")).expect("serializes"),
        "\"linux:900\""
    );
    assert_eq!(
        ProcessStartIdentity::from_token("linux:900").to_string(),
        "linux:900"
    );
}

// =================================================================================================
// The write half — the rename-claim, the four staleness rungs, the tombstone, the sentences
// =================================================================================================

use crate::background::RunId;
use crate::background::reconcile::Liveness;
use std::path::PathBuf;

/// A per-test lease root, resolved the way production resolves it and confined the way §2.4
/// mandates: [`Roots::sandboxed`] under a tempdir, then
/// [`session_leases_root_in`](crate::background::session_leases_root_in).
struct Sandbox {
    tmp: tempfile::TempDir,
    root: PathBuf,
    session: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let roots = crate::paths::Roots::sandboxed(tmp.path());
        let session = tmp.path().join("session.jsonl");
        std::fs::write(&session, b"{}\n").expect("write session file");
        let root = crate::background::session_leases_root_in(&roots);
        Self { tmp, root, session }
    }

    fn lease_dir(&self) -> PathBuf {
        session_lease_dir(&self.session, &self.root).expect("lease dir")
    }

    fn request(&self, run: &str, source: &str, parent: Option<&str>) -> SessionLeaseRequest {
        SessionLeaseRequest {
            session_file: self.session.clone(),
            run_id: RunId::from_token(run.to_string()),
            source_run_id: RunId::from_token(source.to_string()),
            parent_session_id: parent.map(ToString::to_string),
        }
    }
}

const NOW: i64 = 1_700_000_000_000;

/// Options with every ambient input pinned: a fixed clock, a named token, a foreign pid this
/// process does not own, and both probes injected. Nothing here touches the real machine.
fn options(
    token: &'static str,
    pid: u32,
    hostname: &str,
    identity: Option<&str>,
    liveness: fn(u32) -> Liveness,
    start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
) -> SessionLeaseOptions {
    SessionLeaseOptions {
        now: || NOW,
        token: match token {
            "a" => || LeaseToken::from_token("token-a"),
            "b" => || LeaseToken::from_token("token-b"),
            _ => || LeaseToken::from_token("token-c"),
        },
        pid: Some(pid),
        hostname: Some(hostname.to_string()),
        process_start_identity: identity.map(ProcessStartIdentity::from_token),
        liveness,
        start_identity_of,
    }
}

fn never_alive(_: u32) -> Liveness {
    Liveness::Dead
}
fn always_alive(_: u32) -> Liveness {
    Liveness::Alive
}
fn no_identity(_: u32) -> Option<ProcessStartIdentity> {
    None
}

/// Seed a lease held by SOMEONE ELSE, through the real acquire — never by hand-writing the
/// directory. A hand-written incumbent would not exercise the rename-claim that put it there, and
/// every test below is about what happens to a claim that was really made.
async fn seed_incumbent(
    sandbox: &Sandbox,
    options: &SessionLeaseOptions,
    writer: Option<WriterUpdate>,
) -> SessionLeaseHandle {
    let mut handle = acquire_session_lease(
        &sandbox.request("run-1", "run-0", None),
        &sandbox.root,
        options,
    )
    .await
    .expect("the first acquire wins an empty root");
    if let Some(writer) = writer {
        handle.update_writer(writer).await.expect("writer update");
    }
    handle
}

/// pi `:274-277` — the conflict names the record this attempt actually TESTED, not whatever
/// `owner.json` says a moment later.
///
/// Upstream reads the owner ONCE into `existingOwner` and hands that same value to both
/// `demonstrablyStale` and the `SessionLeaseConflictError` it throws. A port that re-reads the
/// file on the refusal arm is byte-correct in both sentences and picks the WRONG one whenever the
/// incumbent releases in between: `SessionLeaseHandle::release` removes the whole lease directory,
/// so the second read finds nothing and the operator is told the lease has *"unreadable owner
/// metadata"* about a lease whose owner had just been read, named, and found alive.
///
/// The race is made deterministic through the ONE seam upstream already has: `isProcessAlive`
/// (`:54`) is called from inside `demonstrablyStale`, strictly between the read and the refusal.
/// The probe here releases the incumbent as its side effect and answers `Alive`, which is exactly
/// the interleaving — and is a real one, because a lease release IS an unlinked directory and the
/// probe IS a syscall the contender makes while holding nothing.
#[tokio::test]
async fn a_lost_claim_names_the_owner_it_tested_even_if_the_lease_is_released_mid_probe() {
    use std::sync::OnceLock;

    /// The lease directory the probe below removes. A `static` because
    /// [`SessionLeaseOptions::liveness`] is a plain `fn` pointer — upstream's own shape (`:54`),
    /// and what keeps the options bag `Clone` and lifetime-free.
    static RELEASE_ON_PROBE: OnceLock<PathBuf> = OnceLock::new();

    fn release_then_report_alive(_: u32) -> Liveness {
        if let Some(dir) = RELEASE_ON_PROBE.get() {
            let _ = std::fs::remove_dir_all(dir);
        }
        Liveness::Alive
    }

    let sandbox = Sandbox::new();
    let incumbent_options = options(
        "a",
        4242,
        "here",
        Some("linux:100"),
        always_alive,
        no_identity,
    );
    let _incumbent = seed_incumbent(&sandbox, &incumbent_options, None).await;
    RELEASE_ON_PROBE
        .set(sandbox.lease_dir())
        .expect("one setter per process; this test owns the static");

    let contender = options(
        "b",
        4243,
        "here",
        Some("linux:100"),
        release_then_report_alive,
        no_identity,
    );
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &contender,
    )
    .await
    .expect_err("a live incumbent is never reclaimable");

    // The lease really is gone by now — the probe removed it — so a re-read would find nothing.
    assert!(
        !sandbox.lease_dir().exists(),
        "the fixture must actually have released the lease mid-probe"
    );
    let SessionLeaseError::Conflict(conflict) = &error else {
        panic!(
            "the refusal must name the owner this attempt read and tested, not report it as \
             unreadable: {error:?}"
        );
    };
    assert_eq!(conflict.run_id.as_str(), "run-1");
    assert_eq!(conflict.pid, 4242);
    assert_eq!(conflict.hostname, "here");
}

/// pi `demonstrablyStale` rung 1 (`:175`). A lease held on ANOTHER machine is never stale HERE,
/// however dead its pid looks from here — this host's pid table says nothing about that host's,
/// and a session file can live on a shared filesystem.
///
/// Gutted (rung 1 dropped): the owner looks dead, this acquirer steals a lease held by a LIVE
/// runner on another machine, and two runners write one session file — VL-S3's exact hazard.
#[tokio::test]
async fn lease_on_a_foreign_hostname_is_never_stale() {
    let sandbox = Sandbox::new();
    let elsewhere = options("a", 4242, "elsewhere", None, never_alive, no_identity);
    let _incumbent = seed_incumbent(&sandbox, &elsewhere, None).await;

    // Here: the same pid reads DEAD, and the writer state is `none` — every rung below rung 1
    // says "stale". Rung 1 must stop it anyway.
    let here = options("b", 4243, "here", None, never_alive, no_identity);
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect_err("a foreign host's lease is never reclaimable from here");
    assert!(is_conflict(&error), "got {error:?}");
    assert!(
        matches!(&error, SessionLeaseError::Conflict(conflict) if conflict.hostname == "elsewhere"),
        "the sentence names the machine that actually holds it: {error}"
    );
    // And the incumbent's directory is untouched — no tombstone was created.
    assert!(sandbox.lease_dir().exists());
    assert_eq!(stale_tombstones(&sandbox), Vec::<PathBuf>::new());
}

/// pi `demonstrablyStale` rung 3 (`:177`) — the unobservable window.
///
/// A writer that has been DISPATCHED but has not yet reported a pid cannot be probed: there is no
/// number to ask about. Upstream refuses to call such a lease stale even when the owner process
/// itself is demonstrably gone, because the child may be mid-fork and about to write the very
/// session file this acquirer wants.
///
/// Gutted (`:177` dropped): a dead owner alone becomes enough, and the lease is stolen out from
/// under a spawning child. `Err` becomes `Ok`.
#[tokio::test]
async fn lease_whose_writer_is_spawning_is_never_stale_even_with_a_dead_owner() {
    let sandbox = Sandbox::new();
    let mine = options("a", 4242, "here", None, never_alive, no_identity);
    let _incumbent = seed_incumbent(&sandbox, &mine, Some(WriterUpdate::Spawning)).await;

    let here = options("b", 4243, "here", None, never_alive, no_identity);
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect_err("a spawning writer withholds the reclaim");
    assert!(is_conflict(&error), "got {error:?}");

    // The SAME lease with `writerState: "none"` IS reclaimable — which is what makes the rung
    // above a rung and not an accident of this fixture.
    let sandbox = Sandbox::new();
    let mine = options("a", 4242, "here", None, never_alive, no_identity);
    let _incumbent = seed_incumbent(&sandbox, &mine, None).await;
    let here = options("b", 4243, "here", None, never_alive, no_identity);
    acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect("a dead owner with no writer is stale");
}

/// pi `processDemonstrablyGone`'s SECOND disjunct (`:169-171`), and the single test that proves
/// the pid-reuse hole is closed.
///
/// The owner's pid is ALIVE. Bare `kill(pid, 0)` therefore says "still running" forever, and the
/// lease would never be reclaimable — every future revival of that session file refused, from one
/// recycled pid. The recorded start identity is what turns "alive" into "alive, and it is a
/// DIFFERENT process".
///
/// Gutted (`:169-171` dropped, i.e. bare liveness): `Ok` becomes `Err(Conflict)`.
#[tokio::test]
async fn lease_alive_at_a_different_start_identity_is_stale() {
    let sandbox = Sandbox::new();
    let mine = options(
        "a",
        4242,
        "here",
        Some("linux:100"),
        always_alive,
        no_identity,
    );
    let incumbent = seed_incumbent(&sandbox, &mine, None).await;
    let incumbent_token = incumbent.token().clone();
    let incumbent_owner = std::fs::read(sandbox.lease_dir().join("owner.json")).expect("owner");

    // The pid is alive — and it is running under a start identity that is not the recorded one.
    fn recycled(_: u32) -> Option<ProcessStartIdentity> {
        Some(ProcessStartIdentity::from_token("linux:999"))
    }
    let here = options("b", 4243, "here", Some("linux:999"), always_alive, recycled);
    let handle = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect("a recycled owner pid makes the lease stale");
    assert_eq!(handle.token().as_str(), "token-b");

    // The stale directory was renamed ASIDE to a PER-OWNER tombstone — never deleted. The name is
    // the INCUMBENT's token, which is what makes every contender that observed this owner target
    // the same occupied destination (pi `:278-280`).
    let tombstone = sandbox.lease_dir().parent().expect("parent").join(format!(
        "{}.stale-{incumbent_token}",
        sandbox
            .lease_dir()
            .file_name()
            .expect("name")
            .to_string_lossy()
    ));
    assert!(
        tombstone.is_dir(),
        "the stale lease is renamed aside, not deleted"
    );
    assert_eq!(
        std::fs::read(tombstone.join("owner.json")).expect("tombstoned owner"),
        incumbent_owner,
        "the evidence survives byte for byte"
    );

    // The counter-rung: the SAME live pid under the SAME recorded identity is NOT stale. Without
    // this half, an over-eager rung reads every alive owner as gone.
    let sandbox = Sandbox::new();
    let mine = options(
        "a",
        4242,
        "here",
        Some("linux:100"),
        always_alive,
        no_identity,
    );
    let _incumbent = seed_incumbent(&sandbox, &mine, None).await;
    fn unchanged(_: u32) -> Option<ProcessStartIdentity> {
        Some(ProcessStartIdentity::from_token("linux:100"))
    }
    let here = options(
        "b",
        4243,
        "here",
        Some("linux:100"),
        always_alive,
        unchanged,
    );
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect_err("an unchanged identity leaves the owner alive");
    assert!(is_conflict(&error), "got {error:?}");
}

/// pi `demonstrablyStale` rung 4 (`:179-180`) — a `running` writer's pid must ALSO be
/// demonstrably gone.
///
/// Gutted (`:179-180` dropped): the lease is reclaimed while the actual writer process is still
/// writing the session file, which is two processes appending to one transcript.
#[tokio::test]
async fn lease_running_writer_requires_both_pids_gone() {
    const OWNER: u32 = 4242;
    const WRITER: u32 = 4777;

    async fn attempt(writer_liveness: fn(u32) -> Liveness) -> Result<(), SessionLeaseError> {
        let sandbox = Sandbox::new();
        let mine = options("a", OWNER, "here", None, never_alive, no_identity);
        let _incumbent =
            seed_incumbent(&sandbox, &mine, Some(WriterUpdate::Running { pid: WRITER })).await;
        let here = options("b", 4243, "here", None, writer_liveness, no_identity);
        acquire_session_lease(
            &sandbox.request("run-2", "run-0", None),
            &sandbox.root,
            &here,
        )
        .await
        .map(|_| ())
    }

    // The owner is gone but the WRITER is alive: not stale.
    fn owner_dead_writer_alive(pid: u32) -> Liveness {
        if pid == WRITER {
            Liveness::Alive
        } else {
            Liveness::Dead
        }
    }
    let error = attempt(owner_dead_writer_alive)
        .await
        .expect_err("a live writer withholds the reclaim");
    assert!(is_conflict(&error), "got {error:?}");

    // Both gone: stale, and reclaimable.
    attempt(never_alive)
        .await
        .expect("a dead owner AND a dead writer make the lease stale");
}

/// DoD 3 — pi `createLeaseDirectory` (`:183-200`). **The rename IS the claim.**
///
/// Eight contenders race for one empty lease directory, with no staleness escape (every owner
/// reads ALIVE). POSIX `rename(2)` refuses to rename a directory onto a non-empty directory,
/// atomically, so exactly ONE may win and the other seven must see the destination occupied.
///
/// Gutted to `create_dir_all`: `mkdir -p` on an existing directory SUCCEEDS, so all eight "win",
/// all eight write `owner.json`, the last writer silently owns a lease seven others believe they
/// hold, and `demonstrablyStale`'s entire contract becomes vacuous. The count below goes from 1
/// to 8.
#[tokio::test]
async fn a_second_acquirer_loses_the_rename_claim() {
    let sandbox = Sandbox::new();
    let root = sandbox.root.clone();
    let session = sandbox.session.clone();

    let mut tasks = Vec::new();
    for index in 0..8u32 {
        let root = root.clone();
        let session = session.clone();
        tasks.push(tokio::spawn(async move {
            let options = SessionLeaseOptions {
                now: || NOW,
                token: || LeaseToken::from_token(uuid::Uuid::new_v4().to_string()),
                pid: Some(5000 + index),
                hostname: Some("here".to_string()),
                process_start_identity: None,
                liveness: always_alive,
                start_identity_of: no_identity,
            };
            let request = SessionLeaseRequest {
                session_file: session,
                run_id: RunId::from_token(format!("run-{index}")),
                source_run_id: RunId::from_token("run-0".to_string()),
                parent_session_id: None,
            };
            acquire_session_lease(&request, &root, &options)
                .await
                .is_ok()
        }));
    }
    let mut winners = 0usize;
    for task in tasks {
        if task.await.expect("the acquire task does not panic") {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "the rename is the claim: exactly one contender may hold it"
    );

    // The losers' candidate directories are cleaned up on every path (pi's `finally`, `:197-199`).
    let siblings: Vec<String> = std::fs::read_dir(&root)
        .expect("read the lease root")
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.contains(".candidate-"))
        .collect();
    assert_eq!(
        siblings,
        Vec::<String>::new(),
        "no candidate directory survives"
    );
}

/// pi `:185`, `:187`, `:189` — 0700 on the directory, 0600 on the record.
///
/// The record names this process's pid, its hostname and the REALPATH of a session transcript.
/// Gutted to default permissions, another user on the box reads all three and can forge an owner
/// record that makes their own lease look like ours.
#[cfg(unix)]
#[tokio::test]
async fn lease_directory_is_0700_and_owner_json_is_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let sandbox = Sandbox::new();
    let mine = options("a", 4242, "here", None, always_alive, no_identity);
    let _handle = seed_incumbent(&sandbox, &mine, None).await;

    let dir_mode = std::fs::metadata(sandbox.lease_dir())
        .expect("lease dir")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "pi `mkdirSync(tempDir, {{ mode: 0o700 }})`"
    );
    let owner_mode = std::fs::metadata(sandbox.lease_dir().join("owner.json"))
        .expect("owner.json")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        owner_mode, 0o600,
        "pi `writeFileSync(..., {{ mode: 0o600 }})`"
    );
}

/// DoD 4 — pi `conflictMessage` (`:154-160`), both sentences, BYTE for byte.
///
/// This is what an operator, or an agent that asked cyrup to revive a session, actually reads when
/// a revival is refused. Gutted to a paraphrase the refusal an agent has to interpret changes, and
/// the `, parent session '…'` clause's exact comma-space placement is the sort of thing a rewrite
/// silently loses.
#[tokio::test]
async fn conflict_sentences_are_byte_identical_to_upstream() {
    // With a parent session.
    let sandbox = Sandbox::new();
    let mine = options("a", 4242, "here", None, always_alive, no_identity);
    let mut handle = acquire_session_lease(
        &sandbox.request("run-owner", "run-source", Some("sess-parent")),
        &sandbox.root,
        &mine,
    )
    .await
    .expect("the first acquire wins");
    let canonical = handle.owner().canonical_session_file.display().to_string();

    let here = options("b", 4243, "here", None, always_alive, no_identity);
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect_err("held");
    assert_eq!(
        error.to_string(),
        format!(
            "Direct revival of session '{canonical}' is already owned by run 'run-owner' (source \
             run 'run-source', parent session 'sess-parent', pid 4242 on here). Wait for that \
             revival to finish or start a separate continuation without reusing this session file."
        )
    );

    // Without one: the clause is absent entirely, not rendered empty.
    assert!(handle.release().await);
    let mine = options("a", 4242, "here", None, always_alive, no_identity);
    let _handle = acquire_session_lease(
        &sandbox.request("run-owner", "run-source", None),
        &sandbox.root,
        &mine,
    )
    .await
    .expect("re-acquired after the release");
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect_err("held");
    assert_eq!(
        error.to_string(),
        format!(
            "Direct revival of session '{canonical}' is already owned by run 'run-owner' (source \
             run 'run-source', pid 4242 on here). Wait for that revival to finish or start a \
             separate continuation without reusing this session file."
        )
    );

    // pi `:156` — a lease directory whose owner cannot be parsed is REFUSED, never reclaimed.
    let sandbox = Sandbox::new();
    let lease_dir = sandbox.lease_dir();
    std::fs::create_dir_all(&lease_dir).expect("mkdir lease dir");
    std::fs::write(lease_dir.join("owner.json"), b"{not json").expect("clobber");
    let canonical = canonical_session_file_path(&sandbox.session)
        .expect("realpath")
        .display()
        .to_string();
    let error = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &here,
    )
    .await
    .expect_err("an unreadable owner is never reclaimed");
    assert_eq!(
        error.to_string(),
        format!(
            "Direct revival of session '{canonical}' is blocked by an existing lease with \
             unreadable owner metadata. Refusing to reclaim it without proof that the owner is \
             stale."
        )
    );
    assert!(is_conflict(&error));
}

/// pi `updateWriter` (`:241-264`) and `release` (`:265-270`) — both re-read and re-check the
/// TOKEN first.
///
/// A handle whose lease was broken as stale and re-taken must not edit, and must not delete, its
/// successor's record. Gutted (the token check dropped), the loser of a staleness race removes the
/// winner's lease on its way out, and a third contender walks in on a session file two runs are
/// writing.
#[tokio::test]
async fn a_handle_whose_lease_was_retaken_refuses_to_write_or_release() {
    let sandbox = Sandbox::new();
    let mine = options("a", 4242, "here", None, never_alive, no_identity);
    let mut handle = seed_incumbent(&sandbox, &mine, None).await;

    // A writer update on a lease this handle still holds lands, and is readable back through the
    // strict validator — which is what makes the `running` staleness rung reachable at all.
    handle
        .update_writer(WriterUpdate::Running { pid: 4777 })
        .await
        .expect("the handle still holds the lease");
    let SessionLeaseState::Owned { owner, .. } =
        inspect_session_lease(&sandbox.session, &sandbox.root).expect("inspect")
    else {
        panic!("the lease is held");
    };
    assert_eq!(
        owner.writer,
        LeaseWriter::Running {
            pid: 4777,
            start_identity: None
        }
    );
    assert_eq!(owner.updated_at_ms, NOW);

    // Now a successor takes it: the owner's pid reads dead, the writer's does too.
    let successor = options("b", 4243, "here", None, never_alive, no_identity);
    let mut successor = acquire_session_lease(
        &sandbox.request("run-2", "run-0", None),
        &sandbox.root,
        &successor,
    )
    .await
    .expect("the stale lease is reclaimed");

    let error = handle
        .update_writer(WriterUpdate::None)
        .await
        .expect_err("the lease is no longer this handle's");
    assert_eq!(
        error.to_string(),
        "Session revival lease ownership changed for run 'run-1'."
    );
    assert!(
        !is_conflict(&error),
        "an ownership change is not a conflict refusal"
    );
    assert!(
        !handle.release().await,
        "a handle that lost its lease must not delete the successor's"
    );
    assert!(
        sandbox.lease_dir().exists(),
        "the successor's lease directory survives the loser's release"
    );

    // And the real holder's release IS acknowledged, which is what the process-terminal
    // candidate records and what `canonical-session-release-unverified` turns on.
    assert!(successor.release().await);
    assert!(!sandbox.lease_dir().exists());
    assert!(
        inspect_session_lease(&sandbox.session, &sandbox.root)
            .expect("inspect")
            .is_free()
    );
}

/// The owner record this build WRITES is the record this build's own strict validator READS.
///
/// A serializer that emitted a `null` for an absent optional, or that wrote `writerPid` beside a
/// non-`running` state, would make this process's own lease read as `unreadable` the moment it
/// tried to reclaim it — a permanent deadlock produced entirely from inside one build.
#[tokio::test]
async fn the_record_this_build_writes_is_the_record_its_validator_reads() {
    let sandbox = Sandbox::new();
    let mine = options(
        "a",
        4242,
        "here",
        Some("linux:100"),
        always_alive,
        no_identity,
    );
    let mut handle = acquire_session_lease(
        &sandbox.request("run-owner", "run-source", Some("sess-parent")),
        &sandbox.root,
        &mine,
    )
    .await
    .expect("acquired");

    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(sandbox.lease_dir().join("owner.json")).expect("owner.json"),
    )
    .expect("valid json");
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["writerState"], "none");
    assert_eq!(raw["acquiredAtMs"], NOW);
    assert_eq!(raw["updatedAtMs"], NOW);
    assert_eq!(raw["acquiredAt"], crate::time::format_iso8601_millis(NOW));
    assert_eq!(raw["processStartIdentity"], "linux:100");
    assert_eq!(raw["parentSessionId"], "sess-parent");
    assert!(raw.get("writerPid").is_none(), "absent, never null: {raw}");
    assert!(
        raw.get("writerProcessStartIdentity").is_none(),
        "absent, never null: {raw}"
    );
    assert!(parse_owner(&raw).is_some(), "the validator accepts it");

    // …and after a `running` update the two writer keys appear, which the same validator requires.
    handle
        .update_writer(WriterUpdate::Running { pid: 4777 })
        .await
        .expect("update");
    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(sandbox.lease_dir().join("owner.json")).expect("owner.json"),
    )
    .expect("valid json");
    assert_eq!(raw["writerState"], "running");
    assert_eq!(raw["writerPid"], 4777);
    assert!(parse_owner(&raw).is_some(), "the validator accepts it");

    // …and disappear again on the way back down, because `parse_owner` refuses a non-`running`
    // state that still carries them (`:142`).
    handle
        .update_writer(WriterUpdate::None)
        .await
        .expect("update");
    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(sandbox.lease_dir().join("owner.json")).expect("owner.json"),
    )
    .expect("valid json");
    assert_eq!(raw["writerState"], "none");
    assert!(
        raw.get("writerPid").is_none(),
        "cleared, not left behind: {raw}"
    );
    assert!(parse_owner(&raw).is_some(), "the validator accepts it");
    drop(sandbox.tmp);
}

/// Upstream's third start-identity rung (`:210-212`) — the runtime fallback, for this process only.
///
/// Without it a platform with no `/proc` records NO identity, `processDemonstrablyGone` degrades
/// to bare liveness, and the pid-reuse hole is back. The value must also be STABLE: two reads that
/// disagreed would make this process's own lease look recycled to itself.
#[tokio::test]
async fn a_procless_owner_still_records_a_stable_runtime_identity() {
    let sandbox = Sandbox::new();
    let self_pid = std::process::id();
    let options = SessionLeaseOptions {
        now: || NOW,
        token: || LeaseToken::from_token("token-a"),
        pid: Some(self_pid),
        hostname: Some("here".to_string()),
        process_start_identity: None,
        // Rung 2 cannot answer — the procless case.
        start_identity_of: no_identity,
        liveness: always_alive,
    };
    let handle = acquire_session_lease(
        &sandbox.request("run-1", "run-0", None),
        &sandbox.root,
        &options,
    )
    .await
    .expect("acquired");
    let recorded = handle
        .owner()
        .process_start_identity
        .as_ref()
        .expect("rung 3 answers for this process");
    assert!(
        recorded.as_str().starts_with("runtime:"),
        "pi's own tag: {recorded}"
    );
    assert_eq!(*recorded, runtime_start_identity(), "and it does not drift");

    // A pid that is NOT this process gets no rung 3 at all — inventing one would be a claim about
    // a process this code has never observed.
    let sandbox = Sandbox::new();
    let foreign = SessionLeaseOptions {
        pid: Some(4242),
        ..options
    };
    let handle = acquire_session_lease(
        &sandbox.request("run-1", "run-0", None),
        &sandbox.root,
        &foreign,
    )
    .await
    .expect("acquired");
    assert!(handle.owner().process_start_identity.is_none());
}

/// Every `.stale-*` sibling of the lease directory, for the tombstone assertions above.
fn stale_tombstones(sandbox: &Sandbox) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(&sandbox.root) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry
                .file_name()
                .to_string_lossy()
                .contains(".stale-")
                .then(|| entry.path())
        })
        .collect()
}

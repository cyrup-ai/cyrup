//! The WRITE half of the lease protocol — pi `session-lease.ts:183-293` @v0.68.0:
//! [`create_lease_directory`]'s rename-claim, [`acquire_session_lease`]'s four attempts with the
//! per-owner stale tombstone, and [`SessionLeaseHandle`]'s two mutations.

use std::path::{Path, PathBuf};

use crate::background::async_retention::lock::{sanitise_stale_key, with_extension_suffix};
use crate::background::atomic::write_private_atomic_json;
use crate::background::reconcile::{Liveness, check_pid_liveness};
use crate::background::result_index::errno;

use super::error::SessionLeaseError;
use super::identity::{ProcessStartIdentity, process_start_identity, runtime_start_identity};
use super::inspect::{OWNER_FILE_NAME, canonical_session_file_path, read_lease_owner_at};
use super::types::{LeaseToken, LeaseWriter, SessionLeaseOwner, SessionLeaseRequest, WriterUpdate};

/// pi's retry budget (`:235`) — `for (let attempt = 0; attempt < 4; attempt++)`.
///
/// Each iteration is one claim attempt plus, on a loss, one tombstone rename. Four is upstream's
/// number and it bounds a genuinely bounded race: every contender that loses either sees a
/// NON-stale owner (which ends the loop immediately with a conflict) or moves the stale directory
/// aside, and only the FIRST can do the latter.
const ACQUIRE_ATTEMPTS: usize = 4;

/// Every ambient input [`acquire_session_lease`] would otherwise read for itself — pi
/// `SessionLeaseOptions` (`:47-56`), minus `rootDir`, which is a REQUIRED argument here (see
/// [`acquire_session_lease`]).
///
/// [`Default`] is production, exactly as upstream's all-optional bag is. Every field is a plain
/// `fn` pointer or an owned value, so the whole struct is cheap and carries no lifetime —
/// the shape [`crate::background::async_retention::lock::RetentionLockIdentity`] already
/// established in this crate.
#[derive(Clone, Debug)]
pub struct SessionLeaseOptions {
    /// pi `now` (`:49`) — the clock `acquiredAtMs`/`updatedAtMs` are stamped from.
    pub now: fn() -> i64,
    /// pi `token` (`:50`) — the per-acquisition token mint.
    pub token: fn() -> LeaseToken,
    /// pi `pid` (`:51`) — who to record as the owner. `None` is this process.
    pub pid: Option<u32>,
    /// pi `hostname` (`:52`) — `None` is
    /// [`machine_hostname`](crate::background::async_retention::machine_hostname), which is this
    /// crate's `os.hostname()`.
    pub hostname: Option<String>,
    /// pi `processStartIdentity` (`:53`) — an identity supplied outright, skipping both fallback
    /// rungs. Present so a test can record an identity for a pid it does not own.
    pub process_start_identity: Option<ProcessStartIdentity>,
    /// pi `isProcessAlive` (`:54`) — rung 2 and rung 4 of the staleness ladder.
    pub liveness: fn(u32) -> Liveness,
    /// pi `getProcessStartIdentity` (`:55`) — the pid-reuse probe, and the source of the owner's
    /// own recorded identity when [`Self::process_start_identity`] is `None`.
    pub start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
}

impl Default for SessionLeaseOptions {
    fn default() -> Self {
        Self {
            now: crate::time::now_epoch_millis,
            // pi `randomUUID()` (`:216`), in upstream's hyphenated rendering — the token becomes
            // part of two sibling directory names, and `-` is inside
            // `sanitise_stale_key`'s safe set, so it survives verbatim.
            token: || LeaseToken::from_token(uuid::Uuid::new_v4().to_string()),
            pid: None,
            hostname: None,
            process_start_identity: None,
            liveness: check_pid_liveness,
            start_identity_of: process_start_identity,
        }
    }
}

/// A HELD lease — pi `SessionLeaseHandle` (`:35-40`).
///
/// Holding one is the claim. There is no `Drop` impl and there deliberately cannot be: both
/// mutations are `async` filesystem operations and Rust has no `async` drop, which is the same
/// constraint [`crate::background::active_async_capacity::claim`] already records for its own
/// handle. Upstream's answer is a `process.once("exit")` net whose own comment names the real
/// fallback — *"Exit cleanup is best effort; a dead-owner lease is reclaimed on the next revival"*
/// (`subagent-runner.ts:5228`) — and that fallback is [`acquire_session_lease`]'s staleness
/// ladder, which works here identically. A runner killed mid-run leaves a lease the NEXT revival
/// reclaims.
#[derive(Debug)]
pub struct SessionLeaseHandle {
    lease_dir: PathBuf,
    owner: SessionLeaseOwner,
    now: fn() -> i64,
    start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
}

impl SessionLeaseHandle {
    /// The owner record as this handle last wrote it (pi `handle.owner`, `:37`).
    #[must_use]
    pub fn owner(&self) -> &SessionLeaseOwner {
        &self.owner
    }

    /// This acquisition's token — the value stamped onto the process-terminal candidate
    /// (`subagent-runner.ts:5183`) and matched by
    /// [`mark_process_terminal_candidate_lease_release`](crate::background::process_terminal::mark_process_terminal_candidate_lease_release).
    #[must_use]
    pub fn token(&self) -> &LeaseToken {
        &self.owner.token
    }

    /// The lease directory this handle holds (pi `handle.leaseDir`, `:36`).
    #[must_use]
    pub fn lease_dir(&self) -> &Path {
        &self.lease_dir
    }

    /// pi `updateWriter` (`:241-264`) — record which process is currently writing the session
    /// file.
    ///
    /// The token is re-read and re-checked FIRST (`:242-245`): a handle whose lease was broken as
    /// stale and re-taken must not edit its successor's record, and the refusal sentence is
    /// upstream's. The writer's start identity is read HERE, from the pid, rather than accepted
    /// from the caller — see [`WriterUpdate`].
    ///
    /// The write itself is atomic (upstream's `createAtomicJsonWriter()`, `:237`/`:260`) and
    /// private: a partially written `owner.json` reads as `unreadable`, which refuses every future
    /// reclaim of that session file until an operator intervenes.
    ///
    /// # Errors
    ///
    /// [`SessionLeaseError::OwnershipChanged`] when the directory no longer holds this token;
    /// [`SessionLeaseError::Claim`] when the atomic write itself fails.
    pub async fn update_writer(&mut self, writer: WriterUpdate) -> Result<(), SessionLeaseError> {
        // pi `:242-245`.
        match read_lease_owner_at(&self.lease_dir) {
            Some(current) if current.token == self.owner.token => {}
            _ => {
                return Err(SessionLeaseError::OwnershipChanged {
                    run_id: self.owner.run_id.clone(),
                });
            }
        }
        // pi `:246-259`. Upstream builds the next owner by spreading, then DELETES both writer
        // keys and re-adds them only for `running` — three lines that exist purely because a
        // JavaScript object spread cannot express "these two fields belong to that state".
        // `LeaseWriter` expresses it, so replacing the field IS the whole update.
        let next = SessionLeaseOwner {
            writer: writer.resolve(self.start_identity_of),
            updated_at_ms: (self.now)(),
            ..self.owner.clone()
        };
        write_private_atomic_json(&self.lease_dir.join(OWNER_FILE_NAME), &next)
            .await
            .map_err(|source| SessionLeaseError::Claim {
                path: self.lease_dir.join(OWNER_FILE_NAME),
                source,
            })?;
        // pi `:261-263` — the in-memory owner tracks the record that was actually written.
        self.owner = next;
        Ok(())
    }

    /// pi `release` (`:265-270`) — drop the claim, and say whether it is DEMONSTRABLY gone.
    ///
    /// The return value is upstream's `!fs.existsSync(leaseDir)`, and it is not cosmetic: it is
    /// the `acknowledged` flag stamped onto the process-terminal candidate
    /// (`subagent-runner.ts:5288`), and `finalizeProcessTerminal` refuses to call a revived run
    /// `observed` without it (`process-terminal.ts:275-276`,
    /// [`ProcessTerminalReason::CanonicalSessionReleaseUnverified`](crate::background::process_terminal::ProcessTerminalReason::CanonicalSessionReleaseUnverified)).
    /// A `false` therefore means "this run's close cannot be proven", which is exactly what an
    /// undeleted lease directory means.
    ///
    /// A lease whose token no longer matches is NOT released and answers `false` (`:267`): the
    /// directory belongs to a successor, and removing it would hand that successor's session file
    /// to a third contender.
    ///
    /// Infallible by contract, like upstream's. There is nothing a caller could do with an error
    /// that the `false` does not already tell it, and the run is over by the time this is called.
    pub async fn release(&mut self) -> bool {
        match read_lease_owner_at(&self.lease_dir) {
            Some(current) if current.token == self.owner.token => {}
            _ => return false,
        }
        let _ = tokio::fs::remove_dir_all(&self.lease_dir).await;
        !tokio::fs::try_exists(&self.lease_dir).await.unwrap_or(true)
    }
}

/// pi `createLeaseDirectory` (`:183-200`) — **the rename IS the claim**.
///
/// # Read this before changing a line of it
///
/// The sequence is: build a private sibling `<lease_dir>.candidate-<token>`, write `owner.json`
/// into it, then `rename` the whole directory onto `<lease_dir>`. POSIX `rename(2)` refuses to
/// rename a directory onto a directory that is NOT EMPTY (`ENOTEMPTY`/`EEXIST`), and it does so
/// atomically — so exactly one of N concurrent contenders can succeed, and every loser observes
/// `lease_dir` already present and returns `false`. `std::fs::rename`, which
/// [`tokio::fs::rename`] wraps, is that syscall on Linux.
///
/// **A `create_dir_all` "simplification" silently destroys the entire mechanism.** `mkdir -p` on
/// an existing directory succeeds, so every contender would "win", every contender would write
/// `owner.json`, and the last writer would own a lease the others believe they hold. The staleness
/// ladder above would then be reasoning about a record that has nothing to do with the process
/// asking. `a_second_acquirer_loses_the_rename_claim` in this module's tests exists to make that
/// edit fail loudly.
///
/// The candidate directory is removed on EVERY exit path, including the successful one (upstream's
/// `finally`, `:197-199`, where the rename has already consumed it and `force: true` makes the
/// removal a no-op).
///
/// # Errors
///
/// A filesystem fault that is not contention. Contention is `Ok(false)`.
async fn create_lease_directory(
    lease_dir: &Path,
    owner: &SessionLeaseOwner,
) -> std::io::Result<bool> {
    let temp_dir = with_extension_suffix(lease_dir, &format!(".candidate-{}", owner.token));
    // pi `:185` — the ROOT is created recursively at 0700; the lease directory itself is created
    // by the rename and inherits the candidate's mode.
    if let Some(parent) = lease_dir.parent() {
        create_private_dir_all(parent).await?;
    }
    // pi `:186` — a candidate left behind by a previous crashed attempt under this same token.
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    // pi `:187` — 0700 from the instant it exists, not a `chmod` a moment later.
    create_private_dir(&temp_dir).await?;

    let outcome = write_candidate_and_claim(lease_dir, &temp_dir, owner).await;
    // pi's `finally` (`:197-199`).
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    outcome
}

/// The body of [`create_lease_directory`]'s `try` (`:188-196`), split out so the `finally` above
/// is a single statement on every path.
async fn write_candidate_and_claim(
    lease_dir: &Path,
    temp_dir: &Path,
    owner: &SessionLeaseOwner,
) -> std::io::Result<bool> {
    // pi `:189` — `writeFileSync(..., { mode: 0o600 })`. NOT an atomic write: the directory
    // rename below is the atomicity, and an atomic writer here would leave its own temp file
    // inside the candidate, which the rename would then carry into the lease directory.
    let bytes = serde_json::to_vec_pretty(owner)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let owner_path = temp_dir.join(OWNER_FILE_NAME);
    write_private_file(&owner_path, &bytes).await?;

    // pi `:191-195` — THE CLAIM.
    match tokio::fs::rename(temp_dir, lease_dir).await {
        Ok(()) => Ok(true),
        Err(error) => {
            // pi `:194` — the destination exists, so someone else holds the lease. That is
            // contention, not a fault.
            if tokio::fs::try_exists(lease_dir).await.unwrap_or(false) {
                Ok(false)
            } else {
                Err(error)
            }
        }
    }
}

/// `mkdir -p` with mode 0700 on every level it creates — pi `:185`'s
/// `mkdirSync(dirname, { recursive: true, mode: 0o700 })`.
async fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(&dir)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// `mkdir` with mode 0700, non-recursive — pi `:187`'s `mkdirSync(tempDir, { mode: 0o700 })`.
///
/// The mode is set through [`std::fs::DirBuilder`] so it holds from the instant the directory
/// exists rather than from a `chmod` a moment later, which is the same reasoning
/// [`crate::background::atomic::write_private_atomic_json_blocking`] records for its `0o600`.
async fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(&dir)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// `writeFileSync(path, bytes, { mode: 0o600 })` (`:189`).
///
/// 0600 from creation. The record names this process's pid, its hostname and the REALPATH of a
/// session transcript; a world-readable copy is an invitation to forge an owner record and steal
/// a lease.
async fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // `tokio::fs::OpenOptions` carries `mode` natively on unix, as
    // `active_async_capacity/claim.rs` already records for its own `0o600` claim file.
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, bytes).await?;
    tokio::io::AsyncWriteExt::flush(&mut file).await
}

/// pi `acquireSessionLease` (`:202-293`) — claim the revival lease on one canonical session file,
/// or refuse with the reason.
///
/// # The protocol, in order
///
/// 1. Resolve the session file to its REALPATH and key the lease directory on its sha256, so two
///    revivals addressing the same file by different paths contend for the SAME directory.
/// 2. Build the owner record — this process's pid, hostname and start identity, a fresh token,
///    `writerState: "none"`.
/// 3. Up to [`ACQUIRE_ATTEMPTS`] times: try [`create_lease_directory`]'s rename-claim. On a win,
///    return the handle. On a loss, read the incumbent owner: if it is unreadable or NOT
///    [`demonstrably_stale`](super::demonstrably_stale), REFUSE with upstream's sentence. If it IS
///    stale, rename the whole directory aside to a PER-OWNER tombstone and go round again.
///
/// # The tombstone is per-OWNER, and upstream's three-line comment (`:278-280`) is the only
/// explanation that exists
///
/// > *The per-owner tombstone is retained. Every contender that observed this stale token targets
/// > the same occupied destination, so only the first can rename it and later contenders cannot
/// > move a successor lease by mistake.*
///
/// Unpacked: the tombstone is named `<lease_dir>.stale-<sanitised owner token>`, which is derived
/// from the record the contender READ, not from itself. So N contenders that all observed the same
/// stale owner all compute the same destination, `rename(2)` lets exactly one of them have it, and
/// the rest see their target already taken and loop — by which time the winner has claimed the
/// lease and they read ITS owner instead. Naming the tombstone after the CONTENDER would give
/// every one of them a free destination, so all N would "break" the lease, and the second one
/// through would move the FIRST one's freshly-claimed successor lease aside. A stale directory
/// is renamed rather than deleted for the same reason the retention lock's is: the evidence
/// survives for an operator, and a delete leaves no record that a lease was broken at all.
///
/// `root_dir` is REQUIRED and has no default, where upstream defaults it to the machine-wide
/// `SESSION_LEASES_DIR` (`:204`). Production names
/// [`session_leases_root_in`](crate::background::session_leases_root_in) at exactly two call
/// sites; with no default there is no path by which a test can reach the shared root by omission,
/// and a leaked lease directory under a shared root poisons every later run on the machine.
///
/// # Errors
///
/// [`SessionLeaseError::Canonicalize`] when the session file cannot be resolved;
/// [`SessionLeaseError::Conflict`] / [`SessionLeaseError::ConflictUnreadableOwner`] when a lease
/// is held and cannot be proven stale; [`SessionLeaseError::Claim`] for a filesystem fault that is
/// neither.
pub async fn acquire_session_lease(
    request: &SessionLeaseRequest,
    root_dir: &Path,
    options: &SessionLeaseOptions,
) -> Result<SessionLeaseHandle, SessionLeaseError> {
    // pi `:203` — the realpath, and the ONE resolution for the whole acquire.
    let canonical_session_file =
        canonical_session_file_path(&request.session_file).map_err(|source| {
            SessionLeaseError::Canonicalize {
                path: request.session_file.clone(),
                source,
            }
        })?;
    let lease_dir =
        super::inspect::session_lease_dir_of_canonical(&canonical_session_file, root_dir);

    let pid = options.pid.unwrap_or_else(std::process::id);
    let hostname = options
        .hostname
        .clone()
        .unwrap_or_else(crate::background::async_retention::machine_hostname);
    // pi `:210-212`, all three rungs: an explicitly supplied identity, then the real probe, then
    // the runtime fallback FOR THIS PROCESS ONLY. Dropping the third rung would leave a non-Linux
    // owner with no identity at all, which degrades `processDemonstrablyGone` to bare liveness and
    // puts the pid-reuse hole back.
    let process_start_identity = options
        .process_start_identity
        .clone()
        .or_else(|| (options.start_identity_of)(pid))
        .or_else(|| (pid == std::process::id()).then(runtime_start_identity));

    let acquired_at_ms = (options.now)();
    let owner = SessionLeaseOwner {
        token: (options.token)(),
        canonical_session_file: canonical_session_file.clone(),
        run_id: request.run_id.as_str().to_string(),
        source_run_id: request.source_run_id.as_str().to_string(),
        parent_session_id: request.parent_session_id.clone(),
        pid,
        hostname: hostname.clone(),
        process_start_identity,
        // pi `:224` — a fresh lease always starts with no writer.
        writer: LeaseWriter::None,
        acquired_at: crate::time::format_iso8601_millis(acquired_at_ms),
        acquired_at_ms,
        updated_at_ms: acquired_at_ms,
    };

    for _ in 0..ACQUIRE_ATTEMPTS {
        let claimed = create_lease_directory(&lease_dir, &owner)
            .await
            .map_err(|source| SessionLeaseError::Claim {
                path: lease_dir.clone(),
                source,
            })?;
        if claimed {
            return Ok(SessionLeaseHandle {
                lease_dir,
                owner,
                now: options.now,
                start_identity_of: options.start_identity_of,
            });
        }

        // pi `:274-277`. Upstream reads `owner.json` ONCE into `existingOwner` and hands that
        // same value to both the staleness test and the conflict it throws (`:276`) — so the
        // sentence the operator reads names the record this attempt actually tested. Matching on
        // the owned value rather than filtering it keeps that: a second
        // `read_lease_owner_at(&lease_dir)` on the refusal arm would race the incumbent's own
        // release (which removes the whole lease directory), and the operator would be told the
        // lease has *"unreadable owner metadata"* about a lease whose owner had just been read
        // and named.
        let existing = read_lease_owner_at(&lease_dir);
        let stale_owner = match existing {
            Some(owner)
                if super::demonstrably_stale(
                    &owner,
                    &hostname,
                    options.liveness,
                    options.start_identity_of,
                ) =>
            {
                owner
            }
            // `None` is an unreadable incumbent; `Some` here is a readable one that is NOT
            // demonstrably stale. Upstream's single `if (!existingOwner || !demonstrablyStale(…))`
            // covers both, and so does this arm.
            incumbent => {
                return Err(SessionLeaseError::conflict(
                    &canonical_session_file,
                    incumbent,
                ));
            }
        };

        // pi `:278-288` — the per-owner tombstone. See this function's doc.
        let tombstone = with_extension_suffix(
            &lease_dir,
            &format!(".stale-{}", sanitise_stale_key(stale_owner.token.as_str())),
        );
        match tokio::fs::rename(&lease_dir, &tombstone).await {
            Ok(()) => {}
            // pi `:286` — `ENOENT` means someone else already moved it; retry.
            Err(error) if errno::is_absent(&error) => continue,
            Err(error) => {
                // pi `:286`'s second disjunct — another contender that observed the SAME stale
                // owner produced this tombstone first. That is a race this attempt lost, not a
                // fault.
                if tokio::fs::try_exists(&tombstone).await.unwrap_or(false) {
                    continue;
                }
                return Err(SessionLeaseError::Claim {
                    path: tombstone,
                    source: error,
                });
            }
        }
    }

    // pi `:291-292` — the budget is spent; report whoever holds it now.
    Err(SessionLeaseError::conflict(
        &canonical_session_file,
        read_lease_owner_at(&lease_dir),
    ))
}

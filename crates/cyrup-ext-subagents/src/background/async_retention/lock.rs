//! The cross-instance retention lock — pi `:378-434`, `:701-705`, `:743-746`, `:902-905`,
//! `:909-911`.
//!
//! # Why a reaper in this crate cannot do without one
//!
//! [`crate::background::run_artifact_roots`] states the premise: *"Running several cyrup instances
//! in one project is ordinary, not an edge case, and they all share these roots byte for byte.
//! Anything written here is therefore visible to all of them, and anything one of them deletes is
//! gone for all of them."* Two unsynchronised passes over that root would race on the same run
//! tree — one renames it onto a tombstone, the other sees a `.deleting-run-*` whose marker names
//! a tree it is itself about to delete — so the pass is serialised per `cwd` scope.
//!
//! # `mkdir` is the acquire
//!
//! [`tokio::fs::create_dir`] on a path that already exists fails with
//! [`std::io::ErrorKind::AlreadyExists`], atomically, on every filesystem this crate runs on. That
//! is upstream's `mkdirSync(lockDir, { mode: 0o700 })` / `EEXIST` pair (`:396-403`) and it needs no
//! advisory-locking syscall, no `flock` (which is per-open-file-description and does not survive
//! the way a directory does), and no `unsafe`.
//!
//! # Staleness is a LADDER, and every rung fails CLOSED
//!
//! A lock that is never broken wedges retention forever the first time a process is `SIGKILL`ed;
//! a lock broken too eagerly lets two passes race. The ladder (`staleLock`, `:378-395`):
//!
//! 1. **unparseable owner** → stale only if the lock directory's own mtime is older than
//!    [`LOCK_STALE_MS`]; an unreadable mtime is NOT stale;
//! 2. **another host** → never stale. A shared scope can live on a network filesystem, where this
//!    machine's pid table says nothing about the owner's;
//! 3. **owner pid confirmed dead** → stale. [`Liveness::Dead`] only —
//!    [`Liveness::Unknown`] (`EPERM` under sandboxing) must NOT be read as dead (R-SA-089), which
//!    is upstream's own `alive === false` vs `alive === undefined` split;
//! 4. **owner pid alive but its start identity changed** → stale. This is pid REUSE: the number is
//!    live, but it is a different process. Rungs 3 and 4 are ONE call —
//!    [`check_pid_identity_with`](crate::background::reconcile::check_pid_identity_with) — shared
//!    with the session lease's `processDemonstrablyGone`, over
//!    [`process_start_identity`](crate::background::session_lease::process_start_identity);
//! 5. otherwise stale only after [`LOCK_STALE_MS`] since the owner took it.
//!
//! # The owner token is re-verified AFTER the work
//!
//! `:743` and `:902`. A pass whose lock was broken out from under it (rung 1 or 5 firing on a
//! slow-but-live owner) must not commit its cursor, because another pass has been making
//! decisions about the same directories since. [`lock_owner_token_matches`] is that check.

use std::path::{Path, PathBuf};

use crate::background::reconcile::{Liveness, check_pid_identity_with, check_pid_liveness};
use crate::background::result_index::errno;
use crate::background::session_lease::{ProcessStartIdentity, process_start_identity};

/// pi `LOCK_NAME` (`:20`) — `<maintenance_root>/.async-retention.lock`, a DIRECTORY.
pub const LOCK_NAME: &str = ".async-retention.lock";

/// The owner record inside the lock directory, `<lock>/owner.json` (`:404`).
const LOCK_OWNER_FILE: &str = "owner.json";

/// pi `LOCK_STALE_MS` (`:26`) — 24 hours.
///
/// The floor under rungs 1 and 5 of the ladder in this module's doc. Deliberately long: every
/// rung above it is evidence-based, and this one is the last resort for a lock whose owner cannot
/// be evaluated at all. A pass delayed by up to a day is a delayed sweep; a lock broken early is a
/// wrong delete.
pub const LOCK_STALE_MS: i64 = 24 * 60 * 60 * 1000;

/// pi `acquireRetentionLock`'s retry budget (`:414`).
const ACQUIRE_ATTEMPTS: usize = 4;

/// Who this process says it is, for the purposes of the lock.
///
/// A value rather than four ambient reads, for the same reason
/// [`crate::registration::doctor::DoctorReportInput`] takes already-resolved inputs: every field
/// here is an environment probe, and a pass that read them itself could not be tested against a
/// foreign host or a dead pid without actually having one.
#[derive(Clone, Debug)]
pub struct RetentionLockIdentity {
    /// This process's pid, written into the owner record so another instance can probe it.
    pub pid: u32,
    /// This machine's hostname. Rung 2 of the ladder compares it against the owner's; a mismatch
    /// makes the lock permanently un-stale from here.
    pub hostname: String,
    /// [`process_start_identity`] for [`Self::pid`], when this platform can answer. `None` simply
    /// disables rung 4 for locks this process takes — never a reason to widen any other rung.
    pub process_start_identity: Option<ProcessStartIdentity>,
    /// The liveness probe rung 3 uses. A plain `fn` pointer rather than a boxed closure because
    /// the only two implementations are [`check_pid_liveness`] and a test's constant, and a
    /// pointer keeps this type [`Clone`] and allocation-free.
    pub liveness: fn(u32) -> Liveness,
    /// The start-identity probe rung 4 uses, injected for the same reason [`Self::liveness`] is:
    /// a test cannot own a pid that has been recycled, so it presents one.
    ///
    /// Rung 4 is
    /// [`check_pid_identity_with`](crate::background::reconcile::check_pid_identity_with)'s
    /// upgrade of an `Alive` answer to [`Liveness::Dead`] — the SAME ladder the session lease's
    /// `processDemonstrablyGone` runs, spelled once in `reconcile` rather than twice here.
    pub start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
}

impl RetentionLockIdentity {
    /// This process, this machine, the real probe.
    #[must_use]
    pub fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            hostname: machine_hostname(),
            process_start_identity: process_start_identity(pid),
            liveness: check_pid_liveness,
            start_identity_of: process_start_identity,
        }
    }
}

/// This machine's hostname, without `unsafe` and without a new dependency.
///
/// This crate is `#![forbid(unsafe_code)]`, so `libc::gethostname` (a raw `*mut c_char` out-param)
/// is not available to it and no workspace crate wraps one. `/proc/sys/kernel/hostname` is the
/// kernel's own answer on Linux — the same string `gethostname(2)` returns, since both read
/// `uts_namespace->name.nodename` — and `/etc/hostname` is the portable-Unix fallback. `HOSTNAME`
/// is consulted last because a shell variable can be stale or absent.
///
/// A machine whose hostname cannot be determined gets the literal `"unknown-host"`. That is the
/// SAFE direction and not a degradation: rung 2 of the ladder compares the owner's hostname with
/// this one, so two processes that both fail to resolve it agree with each other (they really are
/// on the same machine), and a process that resolves it will never break a lock stamped
/// `"unknown-host"` — it will wait out [`LOCK_STALE_MS`] instead.
#[must_use]
pub fn machine_hostname() -> String {
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(raw) = std::fs::read_to_string(path) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown-host".to_string()
}

/// pi `RetentionLockOwner` (`:71-78`) — the `owner.json` record.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionLockOwner {
    /// pi `version: 1`. A record from a future build must FAIL to parse rather than be half-read,
    /// which lands it on rung 1 — the mtime floor — instead of on a misread pid.
    pub version: LockOwnerVersion,
    /// The random token this pass minted. Everything after the acquire compares against it, never
    /// against the pid: a pid is not unique across a break-and-retake.
    pub token: String,
    /// The owner's pid, for rung 3.
    pub pid: u32,
    /// The owner's hostname, for rung 2.
    pub hostname: String,
    /// Epoch millis at acquire, for rung 5.
    pub started_at: i64,
    /// [`process_start_identity`] at acquire, for rung 4. Absent when the platform cannot answer.
    ///
    /// [`ProcessStartIdentity`] is `#[serde(transparent)]` over its token, so this record's
    /// on-disk shape is a plain string exactly as upstream writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_start_identity: Option<ProcessStartIdentity>,
}

/// The owner record's on-disk version, as a TYPE — [`super::TombstoneMarkerVersion`]'s shape and
/// its reasoning: an unknown version must be a parse failure, never a silently-accepted record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockOwnerVersion;

/// The only version this build reads or writes (`:405`).
const LOCK_OWNER_VERSION: u32 = 1;

impl serde::Serialize for LockOwnerVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(LOCK_OWNER_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for LockOwnerVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == LOCK_OWNER_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported retention-lock-owner version {raw} (this build reads version \
                 {LOCK_OWNER_VERSION})"
            )))
        }
    }
}

/// `<maintenance_root>/.async-retention.lock`.
#[must_use]
pub fn lock_dir(maintenance_root: &Path) -> PathBuf {
    maintenance_root.join(LOCK_NAME)
}

/// pi `parseLockOwner` (`:363-376`) — `None` for absent, unreadable or invalid.
pub async fn parse_lock_owner(lock_dir: &Path) -> Option<RetentionLockOwner> {
    let bytes = tokio::fs::read(lock_dir.join(LOCK_OWNER_FILE)).await.ok()?;
    let owner = serde_json::from_slice::<RetentionLockOwner>(&bytes).ok()?;
    // pi's `owner.pid <= 0` guard (`:368`). `u32` already excludes negatives; zero is not a pid.
    if owner.pid == 0 || owner.token.is_empty() {
        return None;
    }
    Some(owner)
}

/// pi `:743`/`:902` — does the lock still belong to `token`?
///
/// Called twice after the destructive work, and a `false` refuses the cursor commit. A pass that
/// lost its lock mid-flight has been making decisions concurrently with whoever now holds it.
pub async fn lock_owner_token_matches(lock_dir: &Path, token: &str) -> bool {
    parse_lock_owner(lock_dir)
        .await
        .is_some_and(|owner| owner.token == token)
}

/// pi `staleLock` (`:378-395`) — the five-rung ladder in this module's doc.
///
/// Returns `(stale, owner_token)`; the token names the stale-aside directory so two instances
/// breaking the same lock do not collide on one tombstone name.
async fn stale_lock(
    lock_dir: &Path,
    now: i64,
    identity: &RetentionLockIdentity,
) -> (bool, Option<String>) {
    let Some(owner) = parse_lock_owner(lock_dir).await else {
        // Rung 1. An unreadable mtime is NOT stale — the same polarity as
        // `SkipReason::UnknownAge`: an age this pass cannot establish is never an old age.
        let stale = match tokio::fs::metadata(lock_dir).await {
            Ok(metadata) => metadata.modified().ok().is_some_and(|mtime| {
                now.saturating_sub(crate::time::epoch_millis(mtime)) >= LOCK_STALE_MS
            }),
            Err(_) => false,
        };
        return (stale, None);
    };
    // Rung 2.
    if owner.hostname != identity.hostname {
        return (false, Some(owner.token));
    }
    // Rungs 3 AND 4, as one ladder: `check_pid_identity_with` is `kill(pid, 0)` (rung 3, `Dead`
    // ONLY — `Unknown` falls through to rung 5, R-SA-089) with the start-identity upgrade on top
    // (rung 4 — the pid is live but recycled). It is the same expression the session lease's
    // `processDemonstrablyGone` uses, so the two subsystems can never disagree about whether a
    // given pid is demonstrably gone.
    if check_pid_identity_with(
        owner.pid,
        owner.process_start_identity.as_ref(),
        identity.liveness,
        identity.start_identity_of,
    ) == Liveness::Dead
    {
        return (true, Some(owner.token));
    }
    // Rung 5.
    (
        now.saturating_sub(owner.started_at) >= LOCK_STALE_MS,
        Some(owner.token),
    )
}

/// pi `createLockDirectory` (`:396-411`) — `mkdir` is the acquire; the owner record follows.
async fn create_lock_directory(
    lock_dir: &Path,
    owner: &RetentionLockOwner,
) -> std::io::Result<bool> {
    match create_private_dir(lock_dir).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    }
    let bytes = serde_json::to_vec(owner)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    match tokio::fs::write(lock_dir.join(LOCK_OWNER_FILE), &bytes).await {
        Ok(()) => Ok(true),
        Err(error) => {
            // pi `:408` — a lock directory with no owner record inside it is unbreakable except by
            // the 24 h floor, so it is removed rather than left behind.
            let _ = tokio::fs::remove_dir_all(lock_dir).await;
            Err(error)
        }
    }
}

/// `mkdir(lockDir, { mode: 0o700 })` (`:398`).
///
/// The mode is set through [`std::fs::DirBuilder`] so it is in effect from the instant the
/// directory exists, rather than a `chmod` a moment later. `cfg(unix)` for the same reason
/// [`crate::background::atomic::write_private_atomic_json_blocking`] guards its own `0o600`:
/// `DirBuilderExt` is a Unix extension trait and Windows has no mode bits to set.
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

/// pi `acquireRetentionLock` (`:413-429`) — at most [`ACQUIRE_ATTEMPTS`] tries.
///
/// Each try is: `mkdir`; on contention evaluate [`stale_lock`]; a lock that is not stale ends the
/// pass immediately (`lock-busy`), and a stale one is renamed ASIDE — never deleted — so the
/// evidence survives for an operator and so two instances breaking the same lock cannot both
/// think they won.
///
/// # Errors
///
/// Propagates a filesystem fault that is neither contention nor an absent lock directory. The
/// caller records it in the report and does not sweep.
pub async fn acquire_retention_lock(
    lock_dir: &Path,
    owner: &RetentionLockOwner,
    identity: &RetentionLockIdentity,
) -> std::io::Result<bool> {
    for _ in 0..ACQUIRE_ATTEMPTS {
        if create_lock_directory(lock_dir, owner).await? {
            return Ok(true);
        }
        // pi passes `owner.startedAt` as `now` (`:417`) — the pass's own clock, not a second read.
        let (stale, stale_token) = stale_lock(lock_dir, owner.started_at, identity).await;
        if !stale {
            return Ok(false);
        }
        let key = sanitise_stale_key(stale_token.as_deref().unwrap_or(&owner.token));
        let aside = with_extension_suffix(lock_dir, &format!(".stale-{key}"));
        match tokio::fs::rename(lock_dir, &aside).await {
            Ok(()) => {}
            Err(error) if errno::is_absent(&error) => continue,
            Err(error) => {
                // pi `:425` — another instance that broke the same lock first already produced the
                // tombstone; that is a race this pass lost, not a fault.
                if tokio::fs::try_exists(&aside).await.unwrap_or(false) {
                    continue;
                }
                return Err(error);
            }
        }
    }
    Ok(false)
}

/// pi `releaseRetentionLock` (`:431-434`) — token-checked, and infallible by contract.
///
/// Upstream's caller wraps this in a `finally` whose own throw is swallowed (`:910`), with the
/// comment *"a stale lock blocks the next pass safely"*. That is the whole error policy: failing
/// to release leaves a lock this host's own liveness probe will break on the next pass, so there
/// is nothing for a caller to do with an error and nothing it could do more safely.
pub async fn release_retention_lock(lock_dir: &Path, token: &str) {
    if !lock_owner_token_matches(lock_dir, token).await {
        return;
    }
    let _ = tokio::fs::remove_dir_all(lock_dir).await;
}

/// pi `:421` — `token.replace(/[^A-Za-z0-9._-]/g, "-")`.
///
/// The token becomes part of a file name, so anything that is not plainly safe in one becomes `-`.
/// This is a path-component guard, not a cosmetic one.
///
/// `pub(crate)` because upstream spells this exact regex twice — here and at
/// `runs/shared/session-lease.ts:281`, for the session lease's own per-owner tombstone — and
/// [`crate::background::session_lease`] calls THIS one rather than carrying a second copy. Two
/// sanitisers that drifted would produce two different names for one token, and the whole point
/// of a per-owner tombstone is that every contender computes the SAME destination.
pub(crate) fn sanitise_stale_key(token: &str) -> String {
    token
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// `${lockDir}.stale-<key>` — a SIBLING of the lock directory, appended to its file name.
///
/// [`Path::with_extension`] would replace `.lock`, producing `.async-retention.stale-<key>` and
/// silently colliding with anything else in that directory; this appends instead.
///
/// `pub(crate)` for the same reason as [`sanitise_stale_key`]: the session lease builds both its
/// `.candidate-<token>` and its `.stale-<token>` siblings with exactly this rule
/// (`session-lease.ts:184`, `:281`), and a lease directory whose name is a 64-character sha256
/// digest has no extension for [`Path::with_extension`] to replace — it would append one and
/// produce a DIFFERENT path than this does.
pub(crate) fn with_extension_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(suffix);
    path.parent().unwrap_or(Path::new(".")).join(name)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn identity(hostname: &str, liveness: fn(u32) -> Liveness) -> RetentionLockIdentity {
        RetentionLockIdentity {
            pid: 4242,
            hostname: hostname.to_string(),
            process_start_identity: Some(ProcessStartIdentity::from_token("linux:1")),
            liveness,
            start_identity_of: process_start_identity,
        }
    }

    fn owner_for(
        identity: &RetentionLockIdentity,
        token: &str,
        started_at: i64,
    ) -> RetentionLockOwner {
        RetentionLockOwner {
            version: LockOwnerVersion,
            token: token.to_string(),
            pid: identity.pid,
            hostname: identity.hostname.clone(),
            started_at,
            process_start_identity: identity.process_start_identity.clone(),
        }
    }

    async fn seed_lock(dir: &Path, owner: &RetentionLockOwner) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        tokio::fs::write(
            dir.join(LOCK_OWNER_FILE),
            serde_json::to_vec(owner).unwrap(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_free_lock_is_acquired_and_released() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let id = identity("here", |_| Liveness::Alive);
        let owner = owner_for(&id, "tok-1", NOW);

        assert!(acquire_retention_lock(&lock, &owner, &id).await.unwrap());
        assert!(lock_owner_token_matches(&lock, "tok-1").await);
        assert!(!lock_owner_token_matches(&lock, "tok-2").await);

        release_retention_lock(&lock, "tok-1").await;
        assert!(!tokio::fs::try_exists(&lock).await.unwrap());
    }

    #[tokio::test]
    async fn a_live_owner_on_this_host_is_never_broken() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let id = identity("here", |_| Liveness::Alive);
        seed_lock(&lock, &owner_for(&id, "held", NOW)).await;

        let mine = owner_for(&id, "mine", NOW);
        assert!(
            !acquire_retention_lock(&lock, &mine, &id).await.unwrap(),
            "a live, same-host owner inside the stale floor keeps the lock"
        );
        assert!(lock_owner_token_matches(&lock, "held").await);
    }

    #[tokio::test]
    async fn a_dead_owner_is_broken_aside_and_the_lock_is_taken() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let alive = identity("here", |_| Liveness::Alive);
        seed_lock(&lock, &owner_for(&alive, "dead-owner", NOW)).await;

        let dead = identity("here", |_| Liveness::Dead);
        let mine = owner_for(&dead, "mine", NOW);
        assert!(acquire_retention_lock(&lock, &mine, &dead).await.unwrap());
        assert!(lock_owner_token_matches(&lock, "mine").await);
        assert!(
            tokio::fs::try_exists(with_extension_suffix(&lock, ".stale-dead-owner"))
                .await
                .unwrap(),
            "the broken lock is renamed aside, never deleted"
        );
    }

    #[tokio::test]
    async fn unknown_liveness_is_not_dead() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let alive = identity("here", |_| Liveness::Alive);
        seed_lock(&lock, &owner_for(&alive, "held", NOW)).await;

        // R-SA-089: an `EPERM`-class probe failure is `Unknown`, and `Unknown` MUST NOT be treated
        // as dead — it falls through to the 24 h floor like any unevaluable owner.
        let unknown = identity("here", |_| Liveness::Unknown);
        let mine = owner_for(&unknown, "mine", NOW);
        assert!(
            !acquire_retention_lock(&lock, &mine, &unknown)
                .await
                .unwrap()
        );

        let stale_mine = owner_for(&unknown, "mine", NOW + LOCK_STALE_MS);
        assert!(
            acquire_retention_lock(&lock, &stale_mine, &unknown)
                .await
                .unwrap(),
            "but the floor still applies once a day has passed"
        );
    }

    #[tokio::test]
    async fn a_lock_owned_by_another_host_is_never_broken() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let elsewhere = identity("elsewhere", |_| Liveness::Dead);
        seed_lock(&lock, &owner_for(&elsewhere, "foreign", NOW)).await;

        // Even with a dead-pid probe AND a year past the floor: this machine's pid table says
        // nothing about another machine's.
        let here = identity("here", |_| Liveness::Dead);
        let mine = owner_for(&here, "mine", NOW + LOCK_STALE_MS * 365);
        assert!(!acquire_retention_lock(&lock, &mine, &here).await.unwrap());
        assert!(lock_owner_token_matches(&lock, "foreign").await);
    }

    #[tokio::test]
    async fn a_reused_pid_makes_the_lock_stale_even_while_the_pid_is_alive() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let id = identity("here", |_| Liveness::Alive);
        // The owner recorded a start identity that this machine's live pid no longer has: the
        // number was recycled onto a different process.
        let mut owner = owner_for(&id, "recycled", NOW);
        owner.pid = std::process::id();
        owner.process_start_identity = Some(ProcessStartIdentity::from_token("linux:0"));
        seed_lock(&lock, &owner).await;

        let mut mine = owner_for(&id, "mine", NOW);
        mine.pid = std::process::id();
        let acquired = acquire_retention_lock(&lock, &mine, &id).await.unwrap();
        if process_start_identity(std::process::id()).is_some() {
            assert!(
                acquired,
                "rung 4 breaks a lock whose owner's pid was reused"
            );
        } else {
            // A platform that cannot answer the probe simply has no rung 4 — never a widened one.
            assert!(!acquired);
        }
    }

    #[tokio::test]
    async fn an_unparseable_owner_waits_out_the_stale_floor() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        tokio::fs::create_dir_all(&lock).await.unwrap();
        tokio::fs::write(lock.join(LOCK_OWNER_FILE), b"{".as_slice())
            .await
            .unwrap();

        let id = identity("here", |_| Liveness::Dead);
        let fresh = owner_for(&id, "mine", crate::time::now_epoch_millis());
        assert!(
            !acquire_retention_lock(&lock, &fresh, &id).await.unwrap(),
            "a fresh lock directory is not stale just because its owner record is unreadable"
        );

        let old = owner_for(
            &id,
            "mine",
            crate::time::now_epoch_millis() + LOCK_STALE_MS + 1,
        );
        assert!(acquire_retention_lock(&lock, &old, &id).await.unwrap());
    }

    #[tokio::test]
    async fn an_owner_record_from_a_future_version_does_not_parse() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        tokio::fs::create_dir_all(&lock).await.unwrap();
        tokio::fs::write(
            lock.join(LOCK_OWNER_FILE),
            br#"{"version":2,"token":"t","pid":1,"hostname":"here","startedAt":0}"#.as_slice(),
        )
        .await
        .unwrap();
        assert!(parse_lock_owner(&lock).await.is_none());
    }

    #[tokio::test]
    async fn releasing_a_lock_this_pass_no_longer_owns_is_a_no_op() {
        let temp = tempfile::tempdir().unwrap();
        let lock = lock_dir(temp.path());
        let id = identity("here", |_| Liveness::Alive);
        seed_lock(&lock, &owner_for(&id, "someone-else", NOW)).await;

        release_retention_lock(&lock, "mine").await;
        assert!(
            lock_owner_token_matches(&lock, "someone-else").await,
            "a pass must never release a lock it does not hold"
        );
    }

    #[test]
    fn a_stale_key_can_never_escape_its_directory() {
        // `.` is in the allowed set (upstream's own class is `[A-Za-z0-9._-]`), so the dots
        // survive and only the separators are replaced — which is all that is needed: a name with
        // no separator cannot address anything but a sibling of the lock.
        let escaped = sanitise_stale_key("../../etc/passwd");
        assert_eq!(escaped, "..-..-etc-passwd");
        assert!(!escaped.contains(std::path::MAIN_SEPARATOR));
        assert_eq!(sanitise_stale_key("ok_1.2-3"), "ok_1.2-3");
        assert_eq!(sanitise_stale_key("a b\0c"), "a-b-c");
    }

    #[test]
    fn the_stale_name_is_a_suffix_and_never_an_extension_replacement() {
        let path = with_extension_suffix(Path::new("/a/.async-retention.lock"), ".stale-x");
        assert_eq!(path, Path::new("/a/.async-retention.lock.stale-x"));
    }

    #[test]
    fn the_hostname_is_a_trimmed_stable_single_token() {
        // Non-emptiness alone is guaranteed by the `"unknown-host"` fallback and so asserts
        // nothing. What rung 2 actually rests on is that the value is STABLE (two processes on one
        // machine must agree) and TRIMMED: `/proc/sys/kernel/hostname` ends in a newline, and an
        // untrimmed hostname would be written into `owner.json` and compared against a trimmed one
        // read from `/etc/hostname` or `$HOSTNAME` on the very next boot path.
        let hostname = machine_hostname();
        assert!(!hostname.is_empty());
        assert_eq!(hostname, hostname.trim(), "no stray newline or padding");
        assert!(
            !hostname.contains(char::is_whitespace),
            "a hostname is one token: {hostname:?}"
        );
        assert_eq!(hostname, machine_hostname(), "and it does not drift");
    }

    #[test]
    fn the_current_process_start_identity_is_stable_and_shaped_as_rung_four_expects() {
        let identity = process_start_identity(std::process::id());
        assert_eq!(
            identity,
            process_start_identity(std::process::id()),
            "two reads of the SAME live pid agree — the property rung 4 rests on"
        );
        if cfg!(target_os = "linux") {
            // On the platform that has the probe, `None` would silently disable rung 4 for every
            // lock this process takes, so the shape is asserted rather than tolerated.
            let identity = identity.expect("Linux answers /proc/<pid>/stat for its own pid");
            let ticks = identity
                .as_str()
                .strip_prefix("linux:")
                .expect("the platform tag upstream also writes");
            assert!(
                ticks.parse::<u64>().is_ok(),
                "field 22 of /proc/<pid>/stat is `starttime`, a tick count: {identity:?}"
            );
        }
    }

    /// A pid this process cannot answer for simply disables rung 4 — it never widens another rung
    /// and it never panics on a `/proc` entry that is not there.
    #[test]
    fn an_unknown_pid_has_no_start_identity() {
        // Pid 0 is not a process on any Unix, so `/proc/0/stat` never exists.
        assert_eq!(process_start_identity(0), None);
    }

    #[tokio::test]
    async fn the_lock_directory_is_created_private() {
        // pi `mkdirSync(lockDir, { mode: 0o700 })` (`:398`). The lock lives in a SHARED scratch
        // root, and its owner record names this process's pid and hostname; world-readable is the
        // wrong default and a `chmod` a moment later would leave a window.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let temp = tempfile::tempdir().unwrap();
            let lock = lock_dir(temp.path());
            let id = identity("here", |_| Liveness::Alive);
            assert!(
                acquire_retention_lock(&lock, &owner_for(&id, "tok-1", NOW), &id)
                    .await
                    .unwrap()
            );
            let mode = tokio::fs::metadata(&lock)
                .await
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o700,
                "the mode is set by DirBuilder, so it holds from the instant the directory exists"
            );
        }
    }
}

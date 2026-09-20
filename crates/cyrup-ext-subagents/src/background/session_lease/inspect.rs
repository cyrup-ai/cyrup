//! Reading a session lease: the canonical key, the lease directory, and
//! [`inspect_session_lease`] — pi `runs/shared/session-lease.ts:96-119`, `:146-152` @v0.68.0.

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::types::{CanonicalSessionId, SessionLeaseState, parse_owner};

/// `owner.json` — the one file a lease directory holds, spelled ONCE for the reader here and the
/// claim that writes it (pi `:189`).
pub(crate) const OWNER_FILE_NAME: &str = "owner.json";

/// pi `canonicalSessionFilePath` (`:96-98`) — `fs.realpathSync.native(path.resolve(sessionFile))`.
///
/// The REALPATH, not the argument: a revival addressed through `~/.cyrup/sessions/x.jsonl` and one
/// addressed through that file's real location must take the SAME lease, or both run and both
/// write the session file — which is the whole hazard the lease exists to prevent.
/// [`std::fs::canonicalize`] is `realpath(3)`, so it resolves every symlink and every `..`.
///
/// # Errors
///
/// The session file does not exist or cannot be resolved. Upstream THROWS here too
/// (`realpathSync` on a missing path), and the callers that must not fail over it — the
/// process-terminal ladder — already catch (`process-terminal.ts:296`).
pub fn canonical_session_file_path(session_file: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(session_file)
}

/// pi `canonicalSessionId` (`:100-104`) — the lowercase sha256 hex of the canonical path.
///
/// Upstream lowercases the key itself on win32 only (`:102`); this build is Unix-only for the
/// lease (the start-identity rung it feeds reads `/proc`), so the key is the path's own bytes.
///
/// # Errors
///
/// [`canonical_session_file_path`]'s.
pub fn canonical_session_id(session_file: &Path) -> std::io::Result<CanonicalSessionId> {
    let canonical = canonical_session_file_path(session_file)?;
    Ok(digest_of(&canonical))
}

/// The digest half, over an ALREADY canonical path — the form
/// [`inspect_session_lease`] uses so one `realpath` call serves the whole inspection (upstream
/// re-enters `canonicalSessionId(canonicalSessionFile)` at `:112`, which is idempotent but a
/// second syscall).
fn digest_of(canonical: &Path) -> CanonicalSessionId {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_os_str().as_encoded_bytes());
    CanonicalSessionId::from_token(hasher.finalize().iter().fold(
        String::with_capacity(64),
        |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        },
    ))
}

/// pi `sessionLeaseDir` (`:106-108`) — `<root_dir>/<canonicalSessionId>`.
///
/// `root_dir` is REQUIRED and has no default, unlike upstream's `rootDir = SESSION_LEASES_DIR`
/// default parameter. Production resolves it once through
/// [`session_leases_root_in`](crate::background::session_leases_root_in); a test passes a
/// `Roots::sandboxed` tempdir, and with no default there is no path by which a test can
/// accidentally reach the shared machine-wide root.
///
/// # Errors
///
/// [`canonical_session_file_path`]'s.
pub fn session_lease_dir(session_file: &Path, root_dir: &Path) -> std::io::Result<PathBuf> {
    Ok(root_dir.join(canonical_session_id(session_file)?.as_str()))
}

/// [`session_lease_dir`] over an ALREADY canonical path — pi's own `sessionLeaseDir(
/// canonicalSessionFile, rootDir)` at `:205`, where `acquireSessionLease` has just resolved the
/// realpath at `:203` and re-entering [`canonical_session_file_path`] would be a second
/// `realpath(2)` on a path that is already one.
#[must_use]
pub(crate) fn session_lease_dir_of_canonical(canonical: &Path, root_dir: &Path) -> PathBuf {
    root_dir.join(digest_of(canonical).as_str())
}

/// pi `readLeaseOwner` (`:146-152`) — read `<lease_dir>/owner.json` and run it through
/// [`parse_owner`]. Any read or parse failure is `None`, which makes the lease `unreadable`.
fn read_lease_owner(lease_dir: &Path) -> Option<super::types::SessionLeaseOwner> {
    let raw = std::fs::read(lease_dir.join(OWNER_FILE_NAME)).ok()?;
    parse_owner(&serde_json::from_slice::<serde_json::Value>(&raw).ok()?)
}

/// pi `inspectSessionLease` (`:110-119`) — the read-only half of the lease protocol.
///
/// Synchronous, like upstream's whole lease module: every call is a `realpath` plus at most one
/// small local read, and both of its callers ([`finalize_process_terminal`](crate::background::process_terminal::finalize_process_terminal)'s
/// lease rung and its `sessionProjection`) run at the very end of a run, off the hot path.
///
/// # Errors
///
/// [`canonical_session_file_path`]'s — a session file that cannot be resolved has no key and
/// therefore no lease to inspect.
pub fn inspect_session_lease(
    session_file: &Path,
    root_dir: &Path,
) -> std::io::Result<SessionLeaseState> {
    let canonical_session_file = canonical_session_file_path(session_file)?;
    let canonical_session_id = digest_of(&canonical_session_file);
    let lease_dir = root_dir.join(canonical_session_id.as_str());
    if !lease_dir.exists() {
        return Ok(SessionLeaseState::Free {
            canonical_session_file,
            canonical_session_id,
        });
    }
    Ok(match read_lease_owner(&lease_dir) {
        Some(owner) => SessionLeaseState::Owned {
            canonical_session_file,
            canonical_session_id,
            owner: Box::new(owner),
        },
        None => SessionLeaseState::Unreadable {
            canonical_session_file,
            canonical_session_id,
        },
    })
}

/// pi `readLeaseOwner` (`:146-152`), for the acquire loop — the same read
/// [`inspect_session_lease`] performs, without re-resolving the session path it already holds.
pub(crate) fn read_lease_owner_at(lease_dir: &Path) -> Option<super::types::SessionLeaseOwner> {
    read_lease_owner(lease_dir)
}

/// pi `demonstrablyStale` (`:174-181`) — the FOUR rungs that decide whether a lease may be
/// reclaimed out from under its recorded owner.
///
/// Every rung fails CLOSED: the default answer is "not stale", and only positive evidence moves
/// it. A lease wrongly declared stale is two runners writing one session file, which is the entire
/// hazard VL-S3 names; a lease wrongly kept is one refused revival with an operator-readable
/// sentence saying exactly who holds it.
///
/// 1. **`owner.hostname != hostname` ⇒ never stale** (`:175`). This machine's pid table says
///    nothing about another machine's, and a session file can live on a shared filesystem. Note
///    the polarity: a foreign host is not "unknown, check the next rung", it is a full stop.
/// 2. **the owner's own pid must be demonstrably gone** (`:176`) — dead, or alive under a
///    DIFFERENT start identity. See [`process_demonstrably_gone`].
/// 3. **`writerState == "spawning"` ⇒ never stale** (`:177`), even with a dead owner. This is the
///    unobservable window: a writer has been dispatched and has not yet reported a pid, so there
///    is no pid to probe and no evidence to be had. Reclaiming here steals the lease from a child
///    that is mid-fork. **This rung is the one most likely to look redundant and is not** — it
///    sits BETWEEN "the owner is gone" and "therefore reclaim", and without it a dead owner alone
///    is enough.
/// 4. **`writerState == "none"` ⇒ stale** (`:178`); **`running` ⇒ the WRITER's pid must ALSO be
///    demonstrably gone** (`:179-180`). A dead owner whose writer is still writing the session
///    file is not a free lease.
///
/// `hostname` and both probes are passed in rather than read here, for the reason upstream passes
/// them (`:174`): a test cannot own a foreign hostname or a recycled pid, so it presents one.
#[must_use]
pub fn demonstrably_stale(
    owner: &super::types::SessionLeaseOwner,
    hostname: &str,
    liveness: fn(u32) -> crate::background::reconcile::Liveness,
    start_identity_of: fn(u32) -> Option<super::identity::ProcessStartIdentity>,
) -> bool {
    use super::identity::process_demonstrably_gone;
    use super::types::LeaseWriter;

    // Rung 1 (`:175`).
    if owner.hostname != hostname {
        return false;
    }
    // Rung 2 (`:176`).
    if !process_demonstrably_gone(
        owner.pid,
        owner.process_start_identity.as_ref(),
        liveness,
        start_identity_of,
    ) {
        return false;
    }
    match &owner.writer {
        // Rung 3 (`:177`).
        LeaseWriter::Spawning => false,
        // Rung 4a (`:178`).
        LeaseWriter::None => true,
        // Rung 4b (`:179-180`). `writerPid` is present BY TYPE here, which is the Rust expression
        // of upstream's `owner.writerPid !== undefined &&` conjunct — and `parse_owner` refuses a
        // `running` record that lacks it, so the conjunct can never be vacuously false for a
        // record this build read off disk.
        LeaseWriter::Running {
            pid,
            start_identity,
        } => process_demonstrably_gone(*pid, start_identity.as_ref(), liveness, start_identity_of),
    }
}

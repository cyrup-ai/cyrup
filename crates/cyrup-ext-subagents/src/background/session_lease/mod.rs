//! The canonical-session revival lease — pi `src/runs/shared/session-lease.ts` @v0.68.0.
//!
//! One directory per CANONICAL SESSION FILE, under
//! [`session_leases_root_in`](crate::background::session_leases_root_in), whose existence is the
//! claim and whose `owner.json` says who holds it. It exists so two revivals of the same session
//! file can never run at once and interleave their writes into it (ledger row VL-S3).
//!
//! # The claim is a `rename`, and every reader must know it
//!
//! [`acquire_session_lease`] builds `<leaseDir>.candidate-<token>`, writes `owner.json` into it,
//! and RENAMES the whole directory onto `<leaseDir>` (`:183-200`). POSIX `rename(2)` refuses to
//! rename a directory onto a non-empty directory, atomically, so exactly one of N concurrent
//! contenders wins and every loser observes `leaseDir` already present. That is why
//! [`inspect_session_lease`]'s "directory exists" test is a meaningful claim test and not merely a
//! cache probe — and it is why a `create_dir_all` "simplification" of
//! `create_lease_directory` would silently destroy the whole mechanism while still compiling.
//!
//! # The four staleness rungs (`:174-181`)
//!
//! A lease that is never broken wedges every future revival of a session file the first time a
//! runner is `SIGKILL`ed; a lease broken too eagerly is two runners writing one session file.
//! [`demonstrably_stale`] is the ladder, and every rung fails CLOSED:
//!
//! 1. **another host ⇒ never stale.** This machine's pid table says nothing about another
//!    machine's, and a session file can live on a shared filesystem.
//! 2. **the owner's own pid must be demonstrably gone** — dead, or alive under a DIFFERENT
//!    [`ProcessStartIdentity`]. Bare liveness is not enough: a recycled pid reads alive forever.
//! 3. **`writerState == "spawning"` ⇒ never stale**, even over a dead owner. The unobservable
//!    window: a writer has been dispatched and has not reported a pid, so there is nothing to
//!    probe.
//! 4. **`none` ⇒ stale; `running` ⇒ the WRITER's pid must ALSO be demonstrably gone.**
//!
//! # The stale tombstone is per-OWNER
//!
//! A lease broken as stale is renamed to `<leaseDir>.stale-<sanitised OWNER token>` — named after
//! the record the contender READ, never after the contender. Upstream's three-line comment
//! (`:278-280`) is the only explanation that exists anywhere and it is reproduced on
//! [`acquire_session_lease`], with the failure mode it prevents spelled out.
//!
//! # Who holds a lease, and for how long
//!
//! ONE process — the RUNNER — from immediately after it loads its config to immediately after its
//! step loop settles, and **only on the revival path** (pi `subagent-runner.ts:5241-5242`, gated
//! on `config.revivalLease`). The orchestrator builds the [`SessionLeaseRequest`]
//! (`extension/executor/control.rs`'s `revive_from_transcript`) and carries it into the runner
//! through `runner-config.json`; the runner acquires, records its per-step writers through
//! [`SessionLeaseHandle::update_writer`], releases, and stamps the acknowledged release onto its
//! process-terminal candidate. A runner killed mid-run leaves a lease the NEXT revival reclaims
//! through the ladder above — which is upstream's own stated fallback (`:5228`).

mod acquire;
mod error;
mod identity;
mod inspect;
mod types;

pub use acquire::{SessionLeaseHandle, SessionLeaseOptions, acquire_session_lease};
pub use error::{SessionLeaseConflict, SessionLeaseError};
pub use identity::{
    ProcessStartIdentity, process_demonstrably_gone, process_start_identity, runtime_start_identity,
};
pub use inspect::{
    canonical_session_file_path, canonical_session_id, demonstrably_stale, inspect_session_lease,
    session_lease_dir,
};
pub use types::{
    CanonicalSessionId, LeaseToken, LeaseWriter, SessionLeaseOwner, SessionLeaseRequest,
    SessionLeaseState, WriterUpdate, parse_owner,
};

#[cfg(test)]
mod tests;

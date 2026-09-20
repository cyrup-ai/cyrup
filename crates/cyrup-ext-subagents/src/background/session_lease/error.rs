//! [`SessionLeaseError`] — pi `SessionLeaseConflictError` (`session-lease.ts:58-66`), its two
//! `conflictMessage` sentences (`:154-160`), `updateWriter`'s throw (`:244`), and the two
//! filesystem faults Rust must name that TypeScript leaves as bare `Error`s.

use std::path::PathBuf;

use super::types::SessionLeaseOwner;

/// The readable-owner conflict's payload — pi `conflictMessage`'s six interpolations
/// (`session-lease.ts:158-159`) plus `SessionLeaseConflictError.owner` (`:59`).
///
/// The owner record travels with the sentence because a caller that wants more than the sentence
/// must not have to re-read a directory that may have changed since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLeaseConflict {
    /// The REALPATH of the contended session file.
    pub canonical_session_file: String,
    /// The holding run.
    pub run_id: String,
    /// The run the holder is reviving from.
    pub source_run_id: String,
    /// Either empty or `, parent session '<id>'` — pi's inline ternary at `:158`, resolved by the
    /// constructor so the format string stays byte-identical to upstream's template.
    pub parent: String,
    /// The holder's pid.
    pub pid: u32,
    /// The holder's machine.
    pub hostname: String,
    /// The parsed owner record, for a caller that wants more than the sentence.
    pub owner: SessionLeaseOwner,
}

/// Every way acquiring or holding a session-revival lease can fail.
///
/// # The three sentences are DELIVERABLES
///
/// [`Self::Conflict`] and [`Self::ConflictUnreadableOwner`] are what an operator — or an agent
/// that asked cyrup to revive a session — actually reads when a revival is refused. They are
/// upstream's `conflictMessage` (`:154-160`) byte for byte, including the `, parent session '…'`
/// clause's exact comma-space placement, because a paraphrase changes the refusal an agent has to
/// interpret. [`Self::OwnershipChanged`] is `:244`'s.
#[derive(Debug, thiserror::Error)]
pub enum SessionLeaseError {
    /// pi `conflictMessage` with a readable owner (`:158-159`) — VERBATIM.
    ///
    /// Boxed as one payload rather than spread across the variant: the six fields the sentence
    /// interpolates plus the owner record are far wider than every other arm, and an un-boxed
    /// variant would make every `Result<_, SessionLeaseError>` in the crate that size
    /// (`clippy::result_large_err`). The format string still names each field, so the sentence is
    /// still readable in one place.
    #[error(
        "Direct revival of session '{}' is already owned by run '{}' (source run '{}'{}, pid {} \
         on {}). Wait for that revival to finish or start a separate continuation without reusing \
         this session file.",
        .0.canonical_session_file, .0.run_id, .0.source_run_id, .0.parent, .0.pid, .0.hostname
    )]
    Conflict(Box<SessionLeaseConflict>),
    /// pi `conflictMessage` with NO readable owner (`:156`) — VERBATIM.
    ///
    /// A lease directory that exists with an unparsable `owner.json` is refused rather than
    /// reclaimed, and the sentence says why: there is no proof the owner is stale, and reclaiming
    /// on no proof is how two runners come to write one session file.
    #[error(
        "Direct revival of session '{canonical_session_file}' is blocked by an existing lease with \
         unreadable owner metadata. Refusing to reclaim it without proof that the owner is stale."
    )]
    ConflictUnreadableOwner {
        /// The REALPATH of the contended session file.
        canonical_session_file: String,
    },
    /// pi `updateWriter`'s throw (`:244`) — VERBATIM.
    ///
    /// The lease directory no longer holds THIS handle's token: it was broken as stale and
    /// re-taken while this process was running. Every subsequent write through the handle is
    /// refused, because it would edit a successor's record.
    #[error("Session revival lease ownership changed for run '{run_id}'.")]
    OwnershipChanged {
        /// The run whose handle went stale.
        run_id: String,
    },
    /// `canonicalSessionFilePath` (`:96-98`) threw: the session file cannot be resolved to a
    /// realpath, so it has no lease key at all.
    ///
    /// Upstream lets `realpathSync` throw out of `acquireSessionLease` unnamed; Rust names it,
    /// because the caller's recovery differs — this is "that file is gone", not "someone else has
    /// it".
    #[error("Failed to resolve canonical session path '{path}': {source}")]
    Canonicalize {
        /// The path as the caller supplied it.
        path: PathBuf,
        /// The underlying resolution failure.
        #[source]
        source: std::io::Error,
    },
    /// `createLeaseDirectory` (`:183-200`) or the stale tombstone rename (`:283`) hit a
    /// filesystem fault that is neither contention nor an absent directory.
    ///
    /// Contention is NOT this error: a lost rename is a `false` return that sends the acquire
    /// round its loop again (`:194`). Only a fault this code cannot reason about lands here, and
    /// a lease this process cannot reason about must not be treated as held.
    #[error("Failed to claim session lease directory '{path}': {source}")]
    Claim {
        /// The lease directory, or the tombstone it was being renamed onto.
        path: PathBuf,
        /// The underlying filesystem failure.
        #[source]
        source: std::io::Error,
    },
}

/// The two arms that ARE pi's `SessionLeaseConflictError` class (`session-lease.ts:58-64`):
/// [`SessionLeaseError::Conflict`] and [`SessionLeaseError::ConflictUnreadableOwner`]. Everything
/// else in this enum is a filesystem fault. There is deliberately no `is_conflict()` predicate —
/// upstream declares the class and never tests `instanceof` it anywhere
/// (`git grep SessionLeaseConflictError v0.68.0 -- src` is four hits, all in `session-lease.ts`
/// itself), and neither does cyrup: the ONE consumer, `runner_main::refuse_run`, refuses the run
/// and writes the sentence for either. A caller that ever needs the distinction matches the two
/// variants, which the compiler then keeps exhaustive.
impl SessionLeaseError {
    /// pi `conflictMessage` (`:154-160`) — the two-sentence split, in ONE place.
    ///
    /// Upstream's function returns a string that is then wrapped in a
    /// `SessionLeaseConflictError`; here the choice of sentence IS the choice of variant, so the
    /// two can never be paired wrongly.
    #[must_use]
    pub fn conflict(
        canonical_session_file: &std::path::Path,
        owner: Option<SessionLeaseOwner>,
    ) -> Self {
        let canonical_session_file = canonical_session_file.display().to_string();
        let Some(owner) = owner else {
            return Self::ConflictUnreadableOwner {
                canonical_session_file,
            };
        };
        // pi `:158` — `owner.parentSessionId ? \`, parent session '${…}'\` : ""`.
        let parent = owner
            .parent_session_id
            .as_ref()
            .map_or_else(String::new, |parent| format!(", parent session '{parent}'"));
        Self::Conflict(Box::new(SessionLeaseConflict {
            canonical_session_file,
            run_id: owner.run_id.clone(),
            source_run_id: owner.source_run_id.clone(),
            parent,
            pid: owner.pid,
            hostname: owner.hostname.clone(),
            owner,
        }))
    }
}

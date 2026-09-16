//! Reconciliation and the snapshot readers.
//!
//! Ports pi `reconcileActiveAsyncCapacity` (`active-async-capacity.ts:338-357`),
//! `getActiveAsyncCapacitySnapshot` (`:359-365`) and `snapshotFor` (`:141-143`) @`v0.66.0`.
//!
//! # Release is reconciliation — §D2
//!
//! Nothing in this module or its siblings offers a `release()`. A slot is removed only here, only
//! against a [`super::ActiveAsyncCapacityReleaseVerdict::Releasable`] verdict, and every
//! [`super::acquire`] runs this first (pi `:460`). So a terminal run's slot is freed by **the next
//! spawn attempt in that session**, or by an explicit reconcile/snapshot read — never by the run
//! finishing. That is why "release on terminal" is stated as an outcome (the next claim sees the
//! slot free) rather than as a call.

use crate::error::SubagentError;
use crate::identity::SessionId;

use super::claim::remove_owned_slot;
use super::config::CapacityOptions;
use super::inspect::owner_release_verdict;
use super::key::{
    ActiveAsyncCapacitySnapshot, occupied_slots, read_owner, session_pool_dir, slot_dir_name,
};

/// pi `snapshotFor` (`:141-143`): how many slots this session's pool currently holds, against the
/// configured cap. `limit: None` renders as `0`, upstream's "the opt-in cap is disabled" sentinel
/// (`shared/types.ts:2202`).
///
/// **Counts, does not reconcile** — the caller decides whether a sweep ran first.
///
/// # Errors
///
/// Any pool-listing failure other than "not found".
pub async fn snapshot_for(
    session_id: &SessionId,
    limit: Option<u32>,
    options: &CapacityOptions,
) -> Result<ActiveAsyncCapacitySnapshot, SubagentError> {
    let pool_dir = session_pool_dir(options.root_dir(), session_id);
    let used = occupied_slots(&pool_dir)
        .await
        .map_err(SubagentError::Spawn)?
        .len();
    Ok(ActiveAsyncCapacitySnapshot {
        used: u32::try_from(used).unwrap_or(u32::MAX),
        limit: limit.unwrap_or(0),
    })
}

/// pi `reconcileActiveAsyncCapacity` (`:338-357`): sweep one session's pool, removing every slot
/// whose owner can be PROVEN released, and report what is left.
///
/// # Three guards decide whether a record is even considered, and all three only ever SKIP
///
/// A record that fails to parse, that names a different session, or that sits in a directory whose
/// name disagrees with its own `slot` field is `continue`d (pi `:344-348`) — **never deleted**.
/// Deleting it would hand the slot to another admission while whatever wrote it may still be
/// running, and the record is the only evidence that it exists. The first two guards live in
/// [`super::key::parse_owner`] and the session comparison below; the third is upstream's
/// `path.basename(dir) !== slot-<n>` check (`:347`), which is a property of the ADDRESS rather
/// than of the record and therefore cannot live in the parser.
///
/// # Errors
///
/// Any pool-listing failure other than "not found" — a pool we cannot list is not a pool we may
/// report as empty, and reporting it empty is how the cap silently stops existing.
pub async fn reconcile_active_async_capacity(
    session_id: &SessionId,
    limit: Option<u32>,
    options: &CapacityOptions,
) -> Result<ActiveAsyncCapacitySnapshot, SubagentError> {
    let pool_dir = session_pool_dir(options.root_dir(), session_id);
    for dir in occupied_slots(&pool_dir)
        .await
        .map_err(SubagentError::Spawn)?
    {
        // pi `:344` — an unparseable or internally inconsistent record (which includes the
        // `ownerSessionKey` cross-check, applied in the parser) is skipped.
        let Some(owner) = read_owner(&dir).await else {
            continue;
        };
        // pi `:345`.
        if owner.owner_session_id != *session_id {
            continue;
        }
        // pi `:347`.
        if dir.file_name().and_then(|name| name.to_str())
            != Some(slot_dir_name(owner.slot).as_str())
        {
            continue;
        }
        let release = owner_release_verdict(&owner, options).await;
        if !release.is_releasable() {
            continue;
        }
        // `require_unstarted` is FALSE here (pi `:352`): reconciliation reclaims against evidence,
        // and the evidence it just gathered is precisely about a run that DID start.
        remove_owned_slot(&dir, &owner, options, false, Some(&release)).await;
    }
    snapshot_for(session_id, limit, options).await
}

/// pi `getActiveAsyncCapacitySnapshot` (`:359-365`) — a bare alias of
/// [`reconcile_active_async_capacity`], kept as its own name because upstream's read sites
/// (`extension/index.ts:929-935`, `doctor.ts:189-192`) call it to *read* while the spawn path calls
/// `reconcile` to *decide*. They are the same operation, and naming both makes it obvious that a
/// "read" here is never free of side effects: it sweeps.
///
/// # Errors
///
/// [`reconcile_active_async_capacity`]'s.
pub async fn get_active_async_capacity_snapshot(
    session_id: &SessionId,
    limit: Option<u32>,
    options: &CapacityOptions,
) -> Result<ActiveAsyncCapacitySnapshot, SubagentError> {
    reconcile_active_async_capacity(session_id, limit, options).await
}

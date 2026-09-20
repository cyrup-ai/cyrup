//! The claim primitive, the slot lifecycle, the handle, and the two admission entry points.
//!
//! Ports pi `active-async-capacity.ts:154-210` (`withSlotClaim`, `removeOwnedSlot`,
//! `appendAbandonedReleaseEvent`), `:367-380` (`createSlot`), `:382-452` (`handleFor`), `:454-480`
//! (`acquireActiveAsyncCapacity`) and `:482-516` (`transferActiveAsyncCapacity`) @`v0.66.0`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::background::{RunDir, RunId, RunState};
use crate::error::SubagentError;
use crate::identity::SessionId;

use super::config::CapacityOptions;
use super::inspect::{
    ActiveAsyncCapacityReleaseEvidence, ActiveAsyncCapacityReleaseVerdict, read_run_status,
};
use super::key::{
    ActiveAsyncCapacityKind, ActiveAsyncCapacityOwner, ActiveAsyncCapacitySnapshot,
    CapacityOwnerVersion, OWNER_FILE, matching_owner, occupied_slots, read_owner, session_pool_dir,
    slot_dir,
};
use super::sweep::{reconcile_active_async_capacity, snapshot_for};

/// The `O_EXCL` lockfile name inside a slot directory (pi `:155`).
const CLAIM_FILE: &str = "capacity.claim";

/// pi `withSlotClaim` (`:154-177`), split into acquire/release because `rollback` and
/// `mark_started` are `async` and Rust has no `async` `finally`.
///
/// # This is an `O_EXCL` lockfile, not an advisory lock — do not "improve" it
///
/// `fs.openSync(claimPath, "wx", 0o600)` translates to
/// `OpenOptions::new().write(true).create_new(true).mode(0o600)`. Two error codes are NOT failures:
/// `EEXIST` (another process holds the claim) and **`ENOENT`** (the slot directory vanished under
/// a concurrent release) — both mean "not acquired". Every other error propagates, because a claim
/// we cannot reason about must not be treated as held.
///
/// [`Self::release`] deletes the claim **only if its contents still match this caller's token**
/// (pi `:173`): a caller whose slot was reclaimed and re-created under a new owner must not delete
/// the NEW holder's claim on its way out.
struct SlotClaim {
    claim_path: PathBuf,
    token: String,
}

impl SlotClaim {
    /// `Ok(None)` for `EEXIST`/`ENOENT`; every other error propagates (pi `:164`).
    async fn acquire(slot_dir: &Path) -> std::io::Result<Option<Self>> {
        let claim_path = slot_dir.join(CLAIM_FILE);
        let token = uuid::Uuid::new_v4().to_string();
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // `tokio::fs::OpenOptions` carries `mode` natively on unix — pi's `0o600` third
        // argument to `fs.openSync(claimPath, "wx", 0o600)` (`:161`).
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = match options.open(&claim_path).await {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotFound
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        use tokio::io::AsyncWriteExt as _;
        file.write_all(token.as_bytes()).await?;
        file.flush().await?;
        drop(file);
        Ok(Some(Self { claim_path, token }))
    }

    /// pi's `finally` (`:171-176`). Best-effort by construction: the slot directory this claim
    /// lives in may legitimately have been renamed away by the very operation that held it, which
    /// is upstream's own `ENOENT` carve-out.
    async fn release(self) {
        if let Ok(bytes) = tokio::fs::read(&self.claim_path).await
            && bytes == self.token.as_bytes()
        {
            let _ = tokio::fs::remove_file(&self.claim_path).await;
        }
    }
}

/// pi `appendAbandonedReleaseEvent` (`:196-210`): one `subagent.capacity.released` line in the
/// run's own `events.jsonl`, so an operator asking "why did my slot disappear" finds the answer
/// beside the run it was taken from.
///
/// Best-effort and swallowed at every step — upstream's bare `catch {}` with the comment *"Capacity
/// release must not fail because its diagnostic event cannot be written."*
async fn append_abandoned_release_event(
    owner: &ActiveAsyncCapacityOwner,
    evidence: ActiveAsyncCapacityReleaseEvidence,
    now: i64,
) {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ReleasedEvent<'a> {
        r#type: &'static str,
        ts: i64,
        run_id: &'a RunId,
        session_id: &'a SessionId,
        #[serde(flatten)]
        evidence: ActiveAsyncCapacityReleaseEvidence,
    }

    let events_path = RunDir::for_existing(&owner.async_dir).events();
    let Some(parent) = events_path.parent() else {
        return;
    };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    let Ok(mut line) = serde_json::to_string(&ReleasedEvent {
        r#type: "subagent.capacity.released",
        ts: now,
        run_id: &owner.run_id,
        session_id: &owner.owner_session_id,
        evidence,
    }) else {
        return;
    };
    line.push('\n');
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .await
    else {
        return;
    };
    use tokio::io::AsyncWriteExt as _;
    let _ = file.write_all(line.as_bytes()).await;
    let _ = file.flush().await;
}

/// pi `removeOwnedSlot` (`:179-194`) — the ONLY way a slot leaves the pool.
///
/// # Rename THEN remove, both steps, always
///
/// The directory is renamed to `.<slot-n>.released-<uuid>` and only then recursively removed, so a
/// concurrent [`occupied_slots`] listing never observes a half-deleted slot: it sees the slot, or
/// it sees nothing, and the dot-prefixed intermediate name fails the `slot-<digits>` predicate so
/// it is never counted either way.
///
/// `require_unstarted` is the reservation/real-run distinction: a slot whose runner has been bound
/// ([`ActiveAsyncCapacityOwner::is_started`]) can be reclaimed by RECONCILIATION against evidence,
/// but never by a rollback.
pub(super) async fn remove_owned_slot(
    dir: &Path,
    expected: &ActiveAsyncCapacityOwner,
    options: &CapacityOptions,
    require_unstarted: bool,
    release: Option<&ActiveAsyncCapacityReleaseVerdict>,
) -> bool {
    // pi `:180` — the cheap in-memory check before the lockfile.
    if require_unstarted && expected.is_started() {
        return false;
    }
    let Ok(Some(claim)) = SlotClaim::acquire(dir).await else {
        return false;
    };
    let removed =
        remove_owned_slot_locked(dir, expected, options, require_unstarted, release).await;
    claim.release().await;
    removed
}

/// The body of [`remove_owned_slot`], run under the claim.
async fn remove_owned_slot_locked(
    dir: &Path,
    expected: &ActiveAsyncCapacityOwner,
    options: &CapacityOptions,
    require_unstarted: bool,
    release: Option<&ActiveAsyncCapacityReleaseVerdict>,
) -> bool {
    // pi `:183` — re-read under the claim; the owner may have changed since the caller decided.
    let Some(current) = matching_owner(dir, expected).await else {
        return false;
    };
    if require_unstarted && current.is_started() {
        return false;
    }
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name().and_then(|n| n.to_str()))
    else {
        return false;
    };
    let released_dir = parent.join(format!(".{name}.released-{}", uuid::Uuid::new_v4()));
    if tokio::fs::rename(dir, &released_dir).await.is_err() {
        return false;
    }
    // pi `:188-190` — the evidence event is written AFTER the slot is out of the pool and BEFORE
    // the tree is removed, so the release is already durable when the diagnostic is attempted.
    if let Some(ActiveAsyncCapacityReleaseVerdict::Releasable {
        evidence: Some(evidence),
        ..
    }) = release
    {
        append_abandoned_release_event(expected, *evidence, options.now()).await;
    }
    let _ = tokio::fs::remove_dir_all(&released_dir).await;
    true
}

/// pi `createSlot` (`:367-380`): `mkdir(slot-N)` as the admission primitive, `EEXIST` meaning "that
/// index is taken, try the next one", and `owner.json` written strictly AFTERWARDS.
///
/// # The ordering is load-bearing and upstream says so
///
/// *"If owner persistence fails, the corrupt occupied directory remains and fails closed instead of
/// becoming available to another admission."* (`:376-377`.) An unparseable `owner.json` therefore
/// keeps the slot OCCUPIED — reconciliation skips a record it cannot parse rather than deleting it
/// — which is the conservative half of the trade. Do not "improve" this into a cleanup.
///
/// # Errors
///
/// A directory-creation failure other than `EEXIST`, or an `owner.json` write failure.
async fn create_slot(pool_dir: &Path, owner: &ActiveAsyncCapacityOwner) -> std::io::Result<bool> {
    let destination = slot_dir(pool_dir, owner.slot);
    tokio::fs::create_dir_all(pool_dir).await?;
    set_private_dir_mode(pool_dir).await;
    match tokio::fs::create_dir(&destination).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    }
    set_private_dir_mode(&destination).await;
    crate::background::atomic::write_private_atomic_json(&destination.join(OWNER_FILE), owner)
        .await?;
    Ok(true)
}

/// pi's `{ mode: 0o700 }` on both `mkdir` calls (`:369`, `:371`). Best-effort: a filesystem that
/// cannot express the mode (or a pre-existing directory owned by this user with a wider mode) must
/// not fail an admission over it, exactly as upstream's `mode` option is advisory against `umask`.
async fn set_private_dir_mode(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// A live claim on one slot — pi `ActiveAsyncCapacityHandle` (`:31-38`).
///
/// # There is no `release()`, and asserting one exists is the bug this type prevents
///
/// A slot is freed **lazily, by reconciliation against evidence** — see [`super`]'s §D2. The
/// handle's whole job between `acquire` and the spawn confirming is to be either PROMOTED
/// ([`Self::mark_started`]) or UNDONE ([`Self::rollback`]).
///
/// # Two upstream methods are deliberately not ported
///
/// * `markWorkflowStarted` (`:398-412`) promotes a `kind: "workflow"` reservation. No cyrup path
///   creates one (see [`ActiveAsyncCapacityKind::Workflow`]), so it would have no call site and
///   no way to be exercised.
/// * `rollbackBeforeRunnerProceed` (`:426-446`) unwinds a reservation during upstream's
///   runner-proceed HANDSHAKE — its runner blocks on `waitForStartupControl`
///   (`subagent-runner.ts:5233-5240`) until the parent writes a proceed token. cyrup has no such
///   barrier: `spawn_detached_runner_with_command` returns with the runner already running and
///   its pid and instance id already bound, so the window that method covers does not exist and
///   there is no call site for it.
#[derive(Debug)]
pub struct ActiveAsyncCapacityHandle {
    owner: ActiveAsyncCapacityOwner,
    limit: u32,
    options: CapacityOptions,
    /// The owner to RESTORE on rollback, for a handle produced by [`transfer`]. `None` for a
    /// freshly acquired slot, whose rollback is a removal (pi's `rollbackOwner` parameter, `:382`).
    rollback_owner: Option<ActiveAsyncCapacityOwner>,
}

impl ActiveAsyncCapacityHandle {
    /// The record as this process last knew it.
    #[must_use]
    pub fn owner(&self) -> &ActiveAsyncCapacityOwner {
        &self.owner
    }

    /// `<pool>/slot-<n>` for this handle (pi `:385`).
    fn dir(&self) -> PathBuf {
        slot_dir(
            &session_pool_dir(self.options.root_dir(), &self.owner.owner_session_id),
            self.owner.slot,
        )
    }

    /// pi `markStarted` (`:388-397`) — bind the reservation to a real runner.
    ///
    /// Binds FOUR facts, in one record: the launch's minted
    /// [`RunnerProcessInstanceId`](crate::background::process_terminal::RunnerProcessInstanceId)
    /// (pi's own `runnerProcessInstanceId`, which the release verdict matches a process-terminal
    /// proof against), the detached runner's OS pid, that pid's
    /// [`ProcessStartIdentity`](crate::background::session_lease::ProcessStartIdentity), and the
    /// bind timestamp. The last three are cyrup's no-proof fallback ladder and are ADDITIVE — a
    /// runner killed before it can write a proof still has a pid whose death is observable, and
    /// the start identity is what stops a RECYCLED pid from reading alive forever.
    ///
    /// Binding is what promotes the slot from "rollbackable reservation" to "real run", which is
    /// exactly the distinction [`remove_owned_slot`]'s `require_unstarted` guard turns on.
    ///
    /// In-memory first, then durable (pi's own comment at `:392-393`), so a failed durable bind is
    /// distinguishable from an unrelated unstarted reservation.
    ///
    /// # Errors
    ///
    /// [`SubagentError::Management`] if ownership of the slot changed under us — upstream throws
    /// the same way (`:396`). If the durable write itself fails the slot is RELEASED before the
    /// error is returned: an owner record with no bind can never be reconciled (the runner
    /// verdict's "runner process identity has not been recorded" rung retains it forever), so
    /// keeping it would leak the slot permanently.
    pub async fn mark_started(
        &mut self,
        pid: u32,
        instance: crate::background::process_terminal::RunnerProcessInstanceId,
    ) -> Result<(), SubagentError> {
        let dir = self.dir();
        let Some(claim) = SlotClaim::acquire(&dir)
            .await
            .map_err(SubagentError::Spawn)?
        else {
            return Err(SubagentError::Management(format!(
                "Active async capacity ownership changed for run '{}'.",
                self.owner.run_id
            )));
        };
        let outcome = self.mark_started_locked(&dir, pid, instance).await;
        claim.release().await;
        match outcome {
            Ok(true) => Ok(()),
            Ok(false) => Err(SubagentError::Management(format!(
                "Active async capacity ownership changed for run '{}'.",
                self.owner.run_id
            ))),
            Err(error) => {
                let expected = self.owner.clone();
                remove_owned_slot(&dir, &expected, &self.options, false, None).await;
                Err(SubagentError::Spawn(error))
            }
        }
    }

    /// The body of [`Self::mark_started`], run under the claim.
    async fn mark_started_locked(
        &mut self,
        dir: &Path,
        pid: u32,
        instance: crate::background::process_terminal::RunnerProcessInstanceId,
    ) -> std::io::Result<bool> {
        let Some(current) = matching_owner(dir, &self.owner).await else {
            return Ok(false);
        };
        let next = ActiveAsyncCapacityOwner {
            runner_process_instance_id: Some(instance),
            runner_pid: Some(pid),
            // Read HERE, at the bind, not at the verdict: the identity is only meaningful paired
            // with the pid it was read for, and reading it later would read whatever process holds
            // that number by then — which is the exact confusion it exists to prevent.
            runner_process_start_identity: crate::background::session_lease::process_start_identity(
                pid,
            ),
            runner_started_at: Some(self.options.now()),
            ..current
        };
        self.owner = next.clone();
        crate::background::atomic::write_private_atomic_json(&dir.join(OWNER_FILE), &next).await?;
        Ok(true)
    }

    /// pi `rollback` (`:414-425`) — undo a reservation that never became a run.
    ///
    /// Two shapes, exactly as upstream: a freshly acquired slot is REMOVED, while a transferred
    /// slot is RESTORED to the owner it was taken from. Both refuse once the runner has been bound
    /// — a started run's slot is reconciliation's to reclaim, never a rollback's.
    ///
    /// Returns whether the rollback actually took effect; a `false` is never fatal to the caller,
    /// which is already on an error path.
    pub async fn rollback(&mut self) -> bool {
        let dir = self.dir();
        let Some(rollback_owner) = self.rollback_owner.clone() else {
            return remove_owned_slot(&dir, &self.owner, &self.options, true, None).await;
        };
        // pi `:416`.
        if self.owner.is_started() {
            return false;
        }
        let Ok(Some(claim)) = SlotClaim::acquire(&dir).await else {
            return false;
        };
        let restored = self.restore_locked(&dir, rollback_owner).await;
        claim.release().await;
        restored
    }

    /// The body of the transfer-rollback arm, run under the claim (pi `:417-424`).
    async fn restore_locked(
        &mut self,
        dir: &Path,
        rollback_owner: ActiveAsyncCapacityOwner,
    ) -> bool {
        match matching_owner(dir, &self.owner).await {
            Some(current) if !current.is_started() => {}
            _ => return false,
        }
        if crate::background::atomic::write_private_atomic_json(
            &dir.join(OWNER_FILE),
            &rollback_owner,
        )
        .await
        .is_err()
        {
            return false;
        }
        self.owner = rollback_owner;
        true
    }

    /// pi `reconcile` (`:448-450`) — sweep this session's pool and report the snapshot, against a
    /// freshly read live-workflow set.
    ///
    /// # Errors
    ///
    /// Any pool-listing failure — see [`reconcile_active_async_capacity`].
    pub async fn reconcile(
        &self,
        live_workflow_run_ids: HashSet<RunId>,
    ) -> Result<ActiveAsyncCapacitySnapshot, SubagentError> {
        let options = self
            .options
            .clone()
            .with_live_workflow_run_ids(live_workflow_run_ids);
        reconcile_active_async_capacity(&self.owner.owner_session_id, Some(self.limit), &options)
            .await
    }
}

/// The five inputs of pi `acquireActiveAsyncCapacity` (`:455`).
#[derive(Clone, Copy, Debug)]
pub struct AcquireInput<'a> {
    /// The session whose pool is charged.
    pub session_id: &'a SessionId,
    /// The resolved cap — pass [`super::resolve_max_active_async_runs_per_session`]'s output.
    /// `None` means UNLIMITED and makes this call touch the filesystem zero times.
    pub limit: Option<u32>,
    /// The run claiming the slot.
    pub run_id: &'a RunId,
    /// What the slot is held for.
    pub kind: ActiveAsyncCapacityKind,
    /// The run directory whose `status.json` every later release verdict reads.
    pub async_dir: &'a Path,
}

/// pi `acquireActiveAsyncCapacity` (`:454-480`): reconcile first, refuse if full, then take the
/// lowest free slot index.
///
/// `Ok(None)` for an unconfigured cap (pi `:456` — `if (input.limit === undefined) return
/// undefined`): an install that never set `maxActiveAsyncRunsPerSession` performs **no** extra
/// filesystem work at all, not even a directory probe.
///
/// The `for slot in 0..limit` scan is upstream's (`:463`): `createSlot`'s `EEXIST` is the race
/// resolver, so two processes admitting simultaneously take different indices without a shared
/// lock. The trailing refusal (`:479`) is reached when every index was taken between the reconcile
/// and the scan — a genuinely full pool, reported from a freshly measured snapshot.
///
/// # Errors
///
/// [`SubagentError::ActiveAsyncCapacityExhausted`] carrying upstream's verbatim message when the
/// session is at its cap; [`SubagentError::Spawn`] for a filesystem failure.
pub async fn acquire(
    input: AcquireInput<'_>,
    options: &CapacityOptions,
) -> Result<Option<ActiveAsyncCapacityHandle>, SubagentError> {
    let Some(limit) = input.limit else {
        return Ok(None);
    };
    // pi `:458` — every acquire reconciles first, which is what makes "release on terminal" an
    // outcome rather than a call (§D2).
    let reconciled =
        reconcile_active_async_capacity(input.session_id, Some(limit), options).await?;
    if reconciled.used >= limit {
        return Err(exhausted(reconciled));
    }
    let pool_dir = session_pool_dir(options.root_dir(), input.session_id);
    let token = uuid::Uuid::new_v4().to_string();
    for slot in 0..limit {
        let owner = ActiveAsyncCapacityOwner {
            version: CapacityOwnerVersion,
            reservation_token: token.clone(),
            owner_session_id: input.session_id.clone(),
            owner_session_key: crate::identity::IndexSegment::encode(input.session_id.as_str())
                .to_string(),
            slot,
            run_id: input.run_id.clone(),
            source_run_id: None,
            generation: 0,
            kind: input.kind,
            async_dir: input.async_dir.to_path_buf(),
            reserved_at: options.now(),
            runner_process_instance_id: None,
            runner_pid: None,
            runner_process_start_identity: None,
            runner_started_at: None,
        };
        if create_slot(&pool_dir, &owner)
            .await
            .map_err(SubagentError::Spawn)?
        {
            return Ok(Some(ActiveAsyncCapacityHandle {
                owner,
                limit,
                options: options.clone(),
                rollback_owner: None,
            }));
        }
    }
    Err(exhausted(
        snapshot_for(input.session_id, Some(limit), options).await?,
    ))
}

/// The five inputs of pi `transferActiveAsyncCapacity` (`:483`).
#[derive(Clone, Copy, Debug)]
pub struct TransferInput<'a> {
    /// The session whose pool holds the source slot.
    pub session_id: &'a SessionId,
    /// The resolved cap, used only to size the handle's later `reconcile`.
    pub limit: Option<u32>,
    /// The run currently holding the slot.
    pub source_run_id: &'a RunId,
    /// The run taking it over.
    pub run_id: &'a RunId,
    /// The NEW run's directory.
    pub async_dir: &'a Path,
}

/// pi `transferActiveAsyncCapacity` (`:482-516`): hand one already-held slot from a settled run to
/// its resume, rather than charging the cap twice for what the operator sees as one run.
///
/// Reached from the spawn gate via
/// [`BackgroundStepsSpec::transfer_from`](crate::extension::BackgroundStepsSpec::transfer_from),
/// whose one producer is `control_resume`'s terminal-revival arm — cyrup's `target.source ==
/// "async"` moment (pi `subagent-executor.ts:2086-2093` @v0.68.0).
///
/// The `generation + 1` bump and the `source_run_id` breadcrumb are what make
/// [`super::inspect_active_async_capacity_owner`]'s `Source` relation mean anything: a stale handle
/// on the old run can no longer mutate the slot, and a caller asking about the old run is told the
/// slot moved rather than that it vanished.
///
/// A source that is still `Queued`/`Running` is a HARD error (`:495-496`), not a fall-through: a
/// live run's slot is not transferable, and silently acquiring a second one would double-charge the
/// session. A source that is simply absent DOES fall through to [`acquire`] (`:513`).
///
/// # Errors
///
/// [`SubagentError::Management`] carrying upstream's message when the source is not transferable or
/// a transfer is already in progress; otherwise [`acquire`]'s errors.
pub async fn transfer(
    input: TransferInput<'_>,
    options: &CapacityOptions,
) -> Result<Option<ActiveAsyncCapacityHandle>, SubagentError> {
    let limit = input.limit.unwrap_or(0);
    let pool_dir = session_pool_dir(options.root_dir(), input.session_id);
    for dir in occupied_slots(&pool_dir)
        .await
        .map_err(SubagentError::Spawn)?
    {
        let Some(source) = read_owner(&dir).await else {
            continue;
        };
        if source.owner_session_id != *input.session_id || source.run_id != *input.source_run_id {
            continue;
        }
        let Some(claim) = SlotClaim::acquire(&dir)
            .await
            .map_err(SubagentError::Spawn)?
        else {
            // pi `:511` — a contended claim here is a concurrent transfer, never a retry.
            return Err(SubagentError::Management(format!(
                "Active async capacity transfer is already in progress for run '{}'.",
                input.source_run_id
            )));
        };
        let transferred = transfer_locked(&dir, &source, &input, limit, options).await;
        claim.release().await;
        return transferred.map(Some);
    }
    // pi `:513` — no slot for the source: this is an ordinary admission.
    acquire(
        AcquireInput {
            session_id: input.session_id,
            limit: input.limit,
            run_id: input.run_id,
            kind: ActiveAsyncCapacityKind::Runner,
            async_dir: input.async_dir,
        },
        options,
    )
    .await
}

/// The body of [`transfer`], run under the claim (pi `:490-509`).
async fn transfer_locked(
    dir: &Path,
    source: &ActiveAsyncCapacityOwner,
    input: &TransferInput<'_>,
    limit: u32,
    options: &CapacityOptions,
) -> Result<ActiveAsyncCapacityHandle, SubagentError> {
    let not_transferable = || {
        SubagentError::Management(format!(
            "Active async capacity source '{}' is not transferable.",
            input.source_run_id
        ))
    };
    let Some(current) = matching_owner(dir, source).await else {
        return Err(not_transferable());
    };
    let status = read_run_status(&source.async_dir).await;
    let transferable = status.is_some_and(|status| {
        status.run_id == *input.source_run_id
            && !matches!(status.state, RunState::Queued | RunState::Running)
    });
    if !transferable {
        return Err(not_transferable());
    }
    let next = ActiveAsyncCapacityOwner {
        run_id: input.run_id.clone(),
        source_run_id: Some(input.source_run_id.clone()),
        generation: current.generation.saturating_add(1),
        kind: ActiveAsyncCapacityKind::Runner,
        async_dir: input.async_dir.to_path_buf(),
        reserved_at: options.now(),
        // pi `delete next.runnerProcessInstanceId; delete next.runnerStartedAt` (`:506-507`): the
        // new run has not started, so the slot is a rollbackable reservation again.
        runner_pid: None,
        runner_started_at: None,
        ..current.clone()
    };
    crate::background::atomic::write_private_atomic_json(&dir.join(OWNER_FILE), &next)
        .await
        .map_err(SubagentError::Spawn)?;
    Ok(ActiveAsyncCapacityHandle {
        owner: next,
        limit,
        options: options.clone(),
        rollback_owner: Some(current),
    })
}

/// pi `ActiveAsyncCapacityError`'s message (`:74`), byte for byte — trailing period included.
fn exhausted(snapshot: ActiveAsyncCapacitySnapshot) -> SubagentError {
    SubagentError::ActiveAsyncCapacityExhausted(format!(
        "Active async run capacity exhausted: {}/{} used.",
        snapshot.used, snapshot.limit
    ))
}

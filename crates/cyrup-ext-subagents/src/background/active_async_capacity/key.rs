//! The on-disk record and its address: [`ActiveAsyncCapacityOwner`], the pool/slot path builders,
//! the owner validator, and the slot enumerator.
//!
//! Ports pi `active-async-capacity.ts:15-29` (the record), `:91-101` (the keys), `:103-128`
//! (`parseOwner`/`readOwner`) and `:130-143` (`occupiedSlots`/`snapshotFor`) @`v0.66.0`.

use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::identity::{IndexSegment, SessionId};

/// The `owner.json` leaf inside every occupied slot directory (pi `:106`, `:124`, `:378`).
pub(crate) const OWNER_FILE: &str = "owner.json";

/// The schema version of an [`ActiveAsyncCapacityOwner`], `1` — pi `version: 1` (`:16`), whose
/// `parseOwner` rejects any other value (`:107`).
///
/// Same unit-type-with-custom-serde shape as
/// [`crate::background::terminal_run_index::TerminalIndexVersion`]: "this record is version 1"
/// becomes a PARSE OUTCOME rather than a field every reader must remember to check, so a slot
/// written by a future build reads back as `None` — which reconciliation already treats as
/// "skip, never delete" — instead of as a panic or a silently misinterpreted record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CapacityOwnerVersion;

impl CapacityOwnerVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for CapacityOwnerVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for CapacityOwnerVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported active-async-capacity owner version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// What a slot is holding capacity FOR — pi `kind: "runner" | "workflow"` (`:24`).
///
/// # `Workflow` has no cyrup acquire site, and that is not dead code
///
/// Upstream's workflow acquire (`subagent-executor.ts:4984` @`v0.66.0`) is gated on
/// `topLevelAsyncWorkflow` — an **async** workflow. cyrup's `route_workflow_mode` refuses the
/// async workflow shape outright (`extension/tool/routing.rs:598-599`: *"`background` is `false` —
/// the async shape was refused above"*), so no cyrup path creates a `Workflow` slot today. The
/// variant is still reached in production through `serde`, by a slot written by a future build or
/// by another cyrup version sharing the session pool, and
/// [`super::inspect::workflow_release_verdict`] runs against it on every reconcile — which is why
/// this module carries no `allow(dead_code)` anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActiveAsyncCapacityKind {
    /// A top-level async RUN, holding the slot from reservation until its runner pid is confirmed
    /// gone and its status is terminal.
    Runner,
    /// A top-level async WORKFLOW shell, holding the slot until its controller is gone and every
    /// async child it launched has a dead runner pid.
    Workflow,
}

/// `{ used, limit }` — pi `ActiveAsyncCapacitySnapshot` (`shared/types.ts:2200-2204` @`v0.66.0`).
///
/// `limit: 0` means the opt-in cap is DISABLED, which is why upstream's own renderer prints
/// `${snapshot.limit || "unlimited"}` (`doctor.ts:195`) rather than the bare number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAsyncCapacitySnapshot {
    /// Occupied slots in this session's pool, after reconciliation. `Default` is
    /// `{ used: 0, limit: 0 }` — the disabled-and-unused snapshot upstream substitutes when there
    /// is no session id at all (`doctor.ts:193`).
    pub used: u32,
    /// The configured cap; **zero means the cap is disabled**.
    pub limit: u32,
}

/// One session's claim on one async-run slot — pi `ActiveAsyncCapacityOwner` (`:15-29`).
///
/// # The one field that is NOT upstream's, and why
///
/// Upstream carries `runnerProcessInstanceId?: string`, minted by its process-terminal protocol
/// (`runs/background/process-terminal.ts`) and the sole positive proof `runnerReleaseVerdict`
/// accepts. **cyrup has neither the artifact nor the identity** — see [`super`]'s module doc §D3 —
/// so this record carries [`Self::runner_pid`] instead: the real OS pid
/// `spawn_detached_runner_with_command` returns, which is the only start-proof cyrup has and is
/// exactly the value [`crate::background::reconcile::check_pid_liveness`] consumes.
/// [`Self::runner_started_at`] is upstream's own field, unchanged.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAsyncCapacityOwner {
    /// Always [`CapacityOwnerVersion::VALUE`]; a record with any other version fails to parse.
    pub version: CapacityOwnerVersion,
    /// pi `reservationToken` (`:17`) — the value every ownership check compares on, so a slot
    /// reclaimed and re-created under another run can never be mutated by the previous holder.
    /// Non-emptiness is enforced by [`parse_owner`].
    pub reservation_token: String,
    /// pi `ownerSessionId` (`:18`). Non-empty BY TYPE ([`SessionId`] deserializes through its own
    /// parser), which is upstream's `!owner.ownerSessionId` guard moved to the parse boundary.
    pub owner_session_id: SessionId,
    /// pi `ownerSessionKey` (`:19`) — the directory key this record claims to live under.
    /// [`parse_owner`] re-derives it from [`Self::owner_session_id`] and refuses a disagreement,
    /// which is what stops one session's pool from being addressable under another's key.
    pub owner_session_key: String,
    /// pi `slot` (`:20`) — the `slot-<n>` index. `>= 0` by type.
    pub slot: u32,
    /// pi `runId` (`:21`) — the run currently holding the slot.
    pub run_id: RunId,
    /// pi `sourceRunId` (`:22`) — the run this slot was TRANSFERRED from, when it was. The
    /// breadcrumb that makes [`super::inspect_active_async_capacity_owner`]'s `source` relation
    /// mean something.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_run_id: Option<RunId>,
    /// pi `generation` (`:23`) — incremented by every transfer, so a stale handle's
    /// token-and-run-id match still fails once the slot has moved on.
    pub generation: u32,
    /// pi `kind` (`:24`).
    pub kind: ActiveAsyncCapacityKind,
    /// pi `asyncDir` (`:25`) — the run directory whose `status.json` every release verdict reads.
    /// Non-emptiness is enforced by [`parse_owner`].
    pub async_dir: PathBuf,
    /// pi `reservedAt` (`:26`) — epoch milliseconds at which the slot was claimed.
    pub reserved_at: i64,
    /// §D3's substitution for pi `runnerProcessInstanceId` (`:27`): the detached runner's real OS
    /// pid, bound by [`super::ActiveAsyncCapacityHandle::mark_started`] once the spawn is
    /// confirmed. Its presence is what promotes the slot from "rollbackable reservation" to "real
    /// run".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_pid: Option<u32>,
    /// pi `runnerStartedAt` (`:28`), unchanged — epoch milliseconds of the bind above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_started_at: Option<i64>,
}

impl ActiveAsyncCapacityOwner {
    /// `true` once a runner has been bound to this reservation — pi's
    /// `owner.runnerProcessInstanceId || owner.runnerStartedAt` (`:180`, `:183`, `:424`), the
    /// exact distinction `requireUnstarted` turns on.
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.runner_pid.is_some() || self.runner_started_at.is_some()
    }
}

/// pi `sessionDir` (`:95-97`) — `<root>/<enc(sessionId)>`.
///
/// ONE key, never [`IndexSegment::read_aliases`]: fanning out over aliases would let one session
/// hold two pools and defeat the cap. See
/// [`crate::background::active_async_capacity_session_dir`], which delegates here so the crate has
/// exactly one copy of this arithmetic.
#[must_use]
pub fn session_pool_dir(root_dir: &Path, session_id: &SessionId) -> PathBuf {
    root_dir.join(IndexSegment::encode(session_id.as_str()).as_str())
}

/// pi `slotDir` (`:99-101`) — `<pool>/slot-<n>`.
#[must_use]
pub fn slot_dir(pool_dir: &Path, slot: u32) -> PathBuf {
    pool_dir.join(format!("slot-{slot}"))
}

/// The `slot-<n>` name a directory must carry to belong to `slot` — pi's
/// `path.basename(dir) !== \`slot-${owner.slot}\`` reconcile guard (`:347`).
#[must_use]
pub fn slot_dir_name(slot: u32) -> String {
    format!("slot-{slot}")
}

/// pi `parseOwner` (`:103-120`): `None` for anything malformed, wrong-versioned, or internally
/// inconsistent.
///
/// serde covers every type check upstream spells out by hand; what serde cannot express, and what
/// is applied here after deserialization, is the two non-empty string guards and the
/// cross-field key check. The remaining upstream guard — `path.basename(dir) === slot-<n>` — is
/// not a property of the record at all and is applied at the reconcile call site
/// ([`super::reconcile_active_async_capacity`], pi `:347`).
///
/// A record that fails any of these is **skipped, never deleted**: upstream `continue`s (`:348`),
/// and deleting it would hand the slot to another admission while whatever wrote it may still be
/// running.
#[must_use]
pub fn parse_owner(bytes: &[u8]) -> Option<ActiveAsyncCapacityOwner> {
    let owner: ActiveAsyncCapacityOwner = serde_json::from_slice(bytes).ok()?;
    if owner.reservation_token.is_empty() || owner.async_dir.as_os_str().is_empty() {
        return None;
    }
    // pi does not re-derive the key (it compares against the session being reconciled, `:346`);
    // deriving it from the record itself is strictly stronger and works for `inspect`, which scans
    // pools it was never given a session id for.
    if owner.owner_session_key != IndexSegment::encode(owner.owner_session_id.as_str()).as_str() {
        return None;
    }
    Some(owner)
}

/// pi `readOwner` (`:122-128`): the slot's `owner.json`, or `None` on ANY error — a missing file,
/// an unreadable one, and an invalid record are all one outcome to every caller.
pub async fn read_owner(slot_dir: &Path) -> Option<ActiveAsyncCapacityOwner> {
    let bytes = tokio::fs::read(slot_dir.join(OWNER_FILE)).await.ok()?;
    parse_owner(&bytes)
}

/// pi `matchingOwner` (`:145-152`): the slot's current owner, but only when token AND run id AND
/// generation all match `expected` — all three, because a transfer bumps the generation while
/// keeping the token, and a reclaim-then-re-create keeps neither.
pub async fn matching_owner(
    slot_dir: &Path,
    expected: &ActiveAsyncCapacityOwner,
) -> Option<ActiveAsyncCapacityOwner> {
    read_owner(slot_dir).await.filter(|owner| {
        owner.reservation_token == expected.reservation_token
            && owner.run_id == expected.run_id
            && owner.generation == expected.generation
    })
}

/// pi `occupiedSlots` (`:130-139`): every `slot-<digits>` DIRECTORY in the pool, in whatever order
/// the filesystem lists them. A missing pool is an empty pool (`ENOENT => []`); every other error
/// propagates, because a pool we cannot list is not a pool we may declare empty — declaring it
/// empty is how the cap silently stops existing.
///
/// # Errors
///
/// Any `read_dir` failure other than "not found".
pub async fn occupied_slots(pool_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut entries = match tokio::fs::read_dir(pool_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut slots = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        // A slot that vanished between the listing and the `file_type` call is simply not there;
        // the same race `withSlotClaim`'s `ENOENT` arm exists for (`:163`).
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if is_slot_dir_name(&name) {
            slots.push(entry.path());
        }
    }
    Ok(slots)
}

/// pi's `/^slot-\d+$/` (`:135`), spelled out: the `.released-<uuid>` rename target and every
/// other stray entry must NOT count against the cap.
fn is_slot_dir_name(name: &str) -> bool {
    name.strip_prefix("slot-")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// pi `capacitySessionDirs` (`:299-309`): the ONE pool for a named session, or every pool under
/// the root when no session was named. A missing root is no pools.
///
/// # Errors
///
/// Any `read_dir` failure other than "not found".
pub async fn capacity_session_dirs(
    root_dir: &Path,
    session_id: Option<&SessionId>,
) -> std::io::Result<Vec<PathBuf>> {
    if let Some(session_id) = session_id {
        return Ok(vec![session_pool_dir(root_dir, session_id)]);
    }
    let mut entries = match tokio::fs::read_dir(root_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut pools = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            pools.push(entry.path());
        }
    }
    Ok(pools)
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

    pub(super) fn owner_json(extra: &str) -> String {
        let key = IndexSegment::encode("s1");
        format!(
            "{{\"version\":1,\"reservationToken\":\"t1\",\"ownerSessionId\":\"s1\",\
             \"ownerSessionKey\":\"{key}\",\"slot\":0,\"runId\":\"r1\",\"generation\":0,\
             \"kind\":\"runner\",\"asyncDir\":\"/a/r1\",\"reservedAt\":5{extra}}}"
        )
    }

    #[test]
    fn a_well_formed_owner_parses_and_round_trips_camel_case() {
        let owner = parse_owner(owner_json("").as_bytes()).expect("valid");
        assert_eq!(owner.run_id.as_str(), "r1");
        assert_eq!(owner.owner_session_id.as_str(), "s1");
        assert_eq!(owner.kind, ActiveAsyncCapacityKind::Runner);
        assert!(!owner.is_started());
        let json = serde_json::to_string(&owner).expect("ser");
        assert!(json.contains("\"ownerSessionKey\""), "got {json}");
        assert!(json.contains("\"version\":1"), "got {json}");
        // The two optional §D3 fields are omitted while unset.
        assert!(!json.contains("runnerPid"), "got {json}");
        assert_eq!(parse_owner(json.as_bytes()).expect("round-trips"), owner);
    }

    #[test]
    fn a_future_version_is_none_not_a_panic() {
        let json = owner_json("").replace("\"version\":1", "\"version\":2");
        assert!(parse_owner(json.as_bytes()).is_none());
    }

    #[test]
    fn an_owner_whose_session_key_disagrees_with_its_session_id_is_unparseable() {
        // Only the KEY moves — the session id stays `s1`, so the two genuinely disagree.
        let json =
            owner_json("").replace("\"ownerSessionKey\":\"s1\"", "\"ownerSessionKey\":\"s2\"");
        assert!(json.contains("\"ownerSessionId\":\"s1\""), "got {json}");
        assert!(parse_owner(json.as_bytes()).is_none());
    }

    #[test]
    fn empty_required_strings_are_unparseable() {
        assert!(parse_owner(owner_json("").replace("\"t1\"", "\"\"").as_bytes()).is_none());
        assert!(parse_owner(owner_json("").replace("\"/a/r1\"", "\"\"").as_bytes()).is_none());
        // An empty session id is refused by `SessionId`'s own Deserialize.
        assert!(
            parse_owner(
                owner_json("")
                    .replace("\"ownerSessionId\":\"s1\"", "\"ownerSessionId\":\"\"")
                    .as_bytes()
            )
            .is_none()
        );
        assert!(parse_owner(b"not json").is_none());
        assert!(parse_owner(b"[]").is_none());
    }

    #[test]
    fn the_started_predicate_fires_on_either_half_of_the_bind() {
        let mut owner = parse_owner(owner_json("").as_bytes()).expect("valid");
        assert!(!owner.is_started());
        owner.runner_pid = Some(4242);
        assert!(owner.is_started());
        owner.runner_pid = None;
        owner.runner_started_at = Some(1);
        assert!(
            owner.is_started(),
            "pi `:180` tests BOTH fields, not just the identity"
        );
    }

    #[test]
    fn only_slot_directories_count_as_occupied() {
        assert!(is_slot_dir_name("slot-0"));
        assert!(is_slot_dir_name("slot-12"));
        assert!(!is_slot_dir_name("slot-"));
        assert!(!is_slot_dir_name("slot-1a"));
        assert!(!is_slot_dir_name("slot"));
        // The rename target `removeOwnedSlot` parks a released slot at must never be counted.
        assert!(!is_slot_dir_name(".slot-0.released-abc"));
        assert!(!is_slot_dir_name("capacity.claim"));
    }

    #[tokio::test]
    async fn occupied_slots_is_empty_for_a_missing_pool_and_skips_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(
            occupied_slots(&tmp.path().join("nope"))
                .await
                .expect("ENOENT is empty")
                .is_empty()
        );
        tokio::fs::create_dir_all(tmp.path().join("slot-0"))
            .await
            .expect("mkdir");
        tokio::fs::write(tmp.path().join("slot-1"), b"")
            .await
            .expect("a FILE named like a slot");
        tokio::fs::write(tmp.path().join("capacity.claim"), b"")
            .await
            .expect("write");
        let slots = occupied_slots(tmp.path()).await.expect("listing");
        assert_eq!(slots.len(), 1, "got {slots:?}");
    }

    #[test]
    fn the_pool_key_is_one_path_component_from_the_shared_encoder() {
        let root = Path::new("/root");
        let sid = SessionId::parse("/home/u/.cyrup/sessions/x.jsonl").expect("non-empty");
        let pool = session_pool_dir(root, &sid);
        assert_eq!(pool.parent(), Some(root), "depth 1 — no traversal");
        let leaf = pool.file_name().and_then(|n| n.to_str()).expect("a leaf");
        // Pinned against an INDEPENDENTLY computed digest rather than against
        // `IndexSegment::encode`: `session_pool_dir` is *defined* as `root.join(encode(id))`, so
        // comparing the two would be an expression against itself and would still pass if the
        // encoder produced a traversable name. This id takes the hash branch because
        // `is_portable_segment` rejects the trailing `.jsonl` extension, and `hashed_segment`
        // hashes the PRE-encoding value, so the expected key is
        // `~sha256-` + sha256("/home/u/.cyrup/sessions/x.jsonl").
        assert_eq!(
            leaf,
            "~sha256-7cfc27445678db1d08b232a529cafbdd5b974ba48e363b5f4f85727872b1277b"
        );
        assert!(
            !leaf.contains(std::path::MAIN_SEPARATOR),
            "a session id that IS a path must not become one: {leaf}"
        );
        // ONE key, never `read_aliases`: the historical URI-encoded alias addresses a DIFFERENT
        // directory, and a pool reachable under both would let one session hold two pools.
        let aliases = IndexSegment::read_aliases(sid.as_str());
        assert_eq!(
            aliases.len(),
            2,
            "this id has a historical alias: {aliases:?}"
        );
        assert!(
            aliases.iter().any(|alias| alias.as_str() != leaf),
            "got {aliases:?}"
        );
    }
}

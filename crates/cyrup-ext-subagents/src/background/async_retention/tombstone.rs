//! The run tombstone's SECOND artefact — the marker — and its whole on-disk contract.
//!
//! # A tombstone is two artefacts, and only one of them is a file
//!
//! 1. **The tombstone itself is the run tree, RENAMED.** pi `:807-811`:
//!    `path.join(asyncDirRoot, RUN_TOMBSTONE_PREFIX + randomId())`, then `renameSync(runDir,
//!    tombstone)`. There is no "tombstone file written next to the run". The rename is the atomic
//!    step that takes the tree out of every other reader's addressable namespace in one syscall —
//!    which is only true because
//!    [`crate::background::terminal_run_index::is_reserved_async_root_entry`] has a
//!    [`RUN_TOMBSTONE_PREFIX`](crate::background::async_retention::RUN_TOMBSTONE_PREFIX) arm.
//!    Performing that rename is SCOPE_14's; this module owns everything the rename needs.
//! 2. **The marker** is a separate small JSON at
//!    `<maintenance_root>/async-retention-run-tombstones/<enc(runId)>.json` — pi
//!    `runTombstoneMarkerPath` (`:213-215`), written with `writeAtomicJson` as
//!    `{ version: 1, runId, tombstonePath, createdAt }` (`:217-221`).
//!
//! The marker exists because after the rename **nothing on disk maps the run id back to the
//! tree**. Two run-side consumers need that mapping: [`marker_matches`], which refuses to reap a
//! `.deleting-run-*` tree whose marker does not resolve to it (pi `:786-788`), and
//! [`read_run_tombstone_marker_self_healing`], whose unlink of a dangling marker (pi `:251`) is
//! what stops markers accumulating forever in a directory this module exists to keep bounded.
//!
//! # Write order is a correctness property
//!
//! The marker write **precedes** the rename (`:808` before `:811`) and is rolled back if the
//! rename fails (`:812-816`). The asymmetry is deliberate: a marker with no tree self-heals at
//! `:251`, whereas a tree with no marker is never reaped (`:787`) and leaks forever. Ordering the
//! two the other way round trades a recoverable state for an unrecoverable one.

use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::background::result_index::errno;
use crate::identity::{IndexSegment, ResultFileName};

use super::RUN_TOMBSTONE_MARKERS_DIR;

/// The marker's on-disk version, as a TYPE rather than a `u32` field.
///
/// Follows [`crate::background::wait_subscriptions::SubscriptionVersion`]: a record written by a
/// future build must FAIL to parse here rather than be half-read. A failed parse is
/// [`TombstoneMarkerState::Unreadable`], which BLOCKS the reap — "I cannot tell" never resolves
/// to "delete it".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TombstoneMarkerVersion;

/// The only version this build reads or writes — pi's `version: 1` (`:218`, `:236`).
const TOMBSTONE_MARKER_VERSION: u32 = 1;

impl serde::Serialize for TombstoneMarkerVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(TOMBSTONE_MARKER_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for TombstoneMarkerVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == TOMBSTONE_MARKER_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported run-tombstone-marker version {raw} (this build reads version \
                 {TOMBSTONE_MARKER_VERSION})"
            )))
        }
    }
}

/// pi's marker record (`writeRunTombstoneMarker`, `:217-221`) — the whole on-disk format.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTombstoneMarker {
    /// pi `version: 1`. See [`TombstoneMarkerVersion`].
    pub version: TombstoneMarkerVersion,
    /// pi `runId` — re-stated inside the record so a marker addressed by
    /// `<enc(runId)>.json` can be checked against the address it was found at (`:236`). A
    /// disagreement is [`TombstoneMarkerState::Unreadable`], not a repair.
    pub run_id: RunId,
    /// pi `tombstonePath` — the `<async_root>/.deleting-run-<id>` tree this marker names.
    pub tombstone_path: PathBuf,
    /// pi `createdAt` — epoch milliseconds, from [`crate::time::now_epoch_millis`] at the call
    /// site. Not consulted by the run-side policy (the grace is measured against the TOMBSTONE
    /// TREE's mtime, `:790`, not against this), and kept because it is the only record of when a
    /// leaked marker was minted.
    pub created_at: i64,
}

/// pi `readRunTombstoneMarker` (`:227-238`) — **three** outcomes, deliberately.
///
/// Collapsing this to two breaks the self-heal: `undefined` → not blocking (`:247`) while
/// `"unreadable"` → blocking (`:248`). It is the same split
/// `completion_replay::retention` states for its own row 2 vs row 4 — *"Row 4 is the one that
/// must not be simplified into row 2"*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TombstoneMarkerState {
    /// No marker file (`ENOENT`, or any errno this crate classes as absent). NOT blocking
    /// (`:247`), and nothing to self-heal.
    Absent,
    /// Present but unusable — not a regular file, a symlink, unreadable, unparseable, the wrong
    /// version, or a `runId` that disagrees with the address it was found at. BLOCKING (`:248`),
    /// and never unlinked: "I cannot tell" must resolve to "leave it alone", in both directions.
    Unreadable,
    /// A valid marker naming a tombstone tree. `tombstone_path` is already normalised by
    /// [`normalize_tombstone_path`], so [`marker_matches`] is a pure comparison.
    Present {
        /// The run id the marker claims, which equals the id its address encodes.
        run_id: RunId,
        /// The `.deleting-run-*` tree, absolutised at read time.
        tombstone_path: PathBuf,
    },
}

/// pi `runTombstoneMarkerPath` (`:213-215`) —
/// `<maintenance_root>/async-retention-run-tombstones/<enc(runId)>.json`.
///
/// The address encoder is [`IndexSegment`] — pi's `encodeIndexSegment` (`:214`) — and
/// [`IndexSegment::encode`], never `encode_bounded` or the alias fan-out: one write key, never
/// the read fan-out, the same discipline [`crate::background::terminal_run_index`] states. It is
/// deliberately NOT [`crate::identity::ResultFileName`]'s raw `encodeURIComponent` family, which
/// is [`crate::background::completion_replay`]'s encoder for a different, round-tripped name.
#[must_use]
pub fn run_tombstone_marker_path(maintenance_root: &Path, run_id: &RunId) -> PathBuf {
    maintenance_root
        .join(RUN_TOMBSTONE_MARKERS_DIR)
        .join(format!(
            "{}{}",
            IndexSegment::encode(run_id.as_str()).as_str(),
            ResultFileName::EXTENSION
        ))
}

/// Absolutise a tombstone path so two spellings of the same tree compare equal.
///
/// pi compares `path.resolve(marker.tombstonePath) === path.resolve(tombstonePath)` (`:242`).
/// cyrup uses [`std::path::absolute`] rather than [`std::fs::canonicalize`] **because the tree may
/// already be gone**: `canonicalize` requires the path to exist, and the one caller that most
/// needs an answer — [`read_run_tombstone_marker_self_healing`] deciding whether a marker is
/// dangling — is asking precisely about a path that may not. `absolute` is lexical (it consults
/// only the process working directory, and only for a relative input), so it answers for a
/// vanished tree, and every path this module handles comes from an already-absolute async root.
/// A failure falls back to the input unchanged, which can only ever make [`marker_matches`]
/// stricter.
#[must_use]
pub fn normalize_tombstone_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// pi `writeRunTombstoneMarker` (`:217-221`) — `mkdir -p` the markers directory, then write the
/// record atomically.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the markers directory cannot be created or made
/// accessible, or if the atomic write fails. The caller (SCOPE_14's sweep) must treat a failure
/// as "do not rename": the marker write PRECEDES the rename precisely so that a failure here
/// leaves the run tree exactly where it was.
pub async fn write_run_tombstone_marker(
    maintenance_root: &Path,
    run_id: &RunId,
    tombstone_path: &Path,
    now: i64,
) -> std::io::Result<()> {
    let marker_path = run_tombstone_marker_path(maintenance_root, run_id);
    if let Some(parent) = marker_path.parent() {
        crate::background::ensure_accessible_dir(parent).await?;
    }
    crate::background::atomic::write_atomic_json(
        &marker_path,
        &RunTombstoneMarker {
            version: TombstoneMarkerVersion,
            run_id: run_id.clone(),
            tombstone_path: normalize_tombstone_path(tombstone_path),
            created_at: now,
        },
    )
    .await
}

/// pi `removeRunTombstoneMarker` (`:223-225`) — `rmSync(..., { force: true })`.
///
/// Infallible by contract, exactly as upstream's `force: true` is: this is called on the rollback
/// path (`:813`) and after a successful reap (`:801`, `:831`), and in both a failure to unlink
/// must not mask the outcome that mattered. A marker that survives is self-healed by
/// [`read_run_tombstone_marker_self_healing`] on the next pass.
pub async fn remove_run_tombstone_marker(maintenance_root: &Path, run_id: &RunId) {
    let _ = tokio::fs::remove_file(run_tombstone_marker_path(maintenance_root, run_id)).await;
}

/// pi `readRunTombstoneMarker` (`:227-238`), verbatim including its validity ladder.
///
/// `lstat` must report a **regular file that is not a symlink** (`:231`); then `version == 1`,
/// `runId` equals the address's run id, and `tombstonePath` is non-empty (`:236`). Every failure
/// but "absent" is [`TombstoneMarkerState::Unreadable`].
pub async fn read_run_tombstone_marker(
    maintenance_root: &Path,
    run_id: &RunId,
) -> TombstoneMarkerState {
    let marker_path = run_tombstone_marker_path(maintenance_root, run_id);
    match tokio::fs::symlink_metadata(&marker_path).await {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return TombstoneMarkerState::Unreadable;
            }
        }
        // pi `isNotFound(error) ? undefined : "unreadable"` (`:233`).
        Err(error) if errno::is_absent(&error) => return TombstoneMarkerState::Absent,
        Err(_) => return TombstoneMarkerState::Unreadable,
    }
    let Ok(bytes) = tokio::fs::read(&marker_path).await else {
        return TombstoneMarkerState::Unreadable;
    };
    let Ok(marker) = serde_json::from_slice::<RunTombstoneMarker>(&bytes) else {
        return TombstoneMarkerState::Unreadable;
    };
    if marker.run_id != *run_id || marker.tombstone_path.as_os_str().is_empty() {
        return TombstoneMarkerState::Unreadable;
    }
    TombstoneMarkerState::Present {
        run_id: marker.run_id,
        tombstone_path: normalize_tombstone_path(&marker.tombstone_path),
    }
}

/// pi `runTombstoneMarkerBlocks` (`:245-256`) — [`read_run_tombstone_marker`] PLUS the self-heal
/// that keeps the markers directory from growing without bound.
///
/// Upstream returns a bool; this returns the state, because the run-side caller needs the state
/// anyway ([`marker_matches`]) and the bool is recoverable from it — a marker blocks iff the
/// result is not [`TombstoneMarkerState::Absent`].
///
/// The three arms map exactly onto `:247-255`:
///
/// * [`TombstoneMarkerState::Absent`] → not blocking, nothing to heal;
/// * [`TombstoneMarkerState::Unreadable`] → blocking, and **never unlinked**;
/// * [`TombstoneMarkerState::Present`] → blocking while the tree exists; when the tree is gone
///   the marker is dangling, so it is unlinked and the answer becomes `Absent` (`:250-252`). A
///   stat failure that is not "absent" leaves the marker in place and keeps blocking (`:254`).
pub async fn read_run_tombstone_marker_self_healing(
    maintenance_root: &Path,
    run_id: &RunId,
) -> TombstoneMarkerState {
    let state = read_run_tombstone_marker(maintenance_root, run_id).await;
    let TombstoneMarkerState::Present { tombstone_path, .. } = &state else {
        return state;
    };
    match tokio::fs::symlink_metadata(tombstone_path).await {
        Ok(_) => state,
        Err(error) if errno::is_absent(&error) => {
            remove_run_tombstone_marker(maintenance_root, run_id).await;
            TombstoneMarkerState::Absent
        }
        Err(_) => state,
    }
}

/// pi `runTombstoneMarkerMatches` (`:240-243`) — does this marker resolve to THIS tree?
///
/// Pure: both sides are already absolutised ([`normalize_tombstone_path`] at read time, and
/// [`crate::background::async_retention::scan_run_candidates`] for the candidate's own directory),
/// so this is a comparison and nothing more — which is what lets
/// [`crate::background::async_retention::decide`] stay free of I/O.
///
/// `Absent` and `Unreadable` both answer `false`, exactly as `:242`'s two `!==` guards do: an
/// existing `.deleting-run-*` tree whose marker cannot be resolved to it is never reaped.
#[must_use]
pub fn marker_matches(
    state: &TombstoneMarkerState,
    run_id: &RunId,
    absolute_tombstone_path: &Path,
) -> bool {
    match state {
        TombstoneMarkerState::Present {
            run_id: marker_run_id,
            tombstone_path,
        } => marker_run_id == run_id && tombstone_path == absolute_tombstone_path,
        TombstoneMarkerState::Absent | TombstoneMarkerState::Unreadable => false,
    }
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

    fn run(token: &str) -> RunId {
        RunId::from_token(token.to_string())
    }

    #[tokio::test]
    async fn a_tombstone_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let maintenance = temp.path().join(".async-retention");
        let run_id = run("0123456789abcdef0123456789abcdef");
        let tombstone = temp.path().join(".deleting-run-abc");

        assert_eq!(
            read_run_tombstone_marker(&maintenance, &run_id).await,
            TombstoneMarkerState::Absent,
            "no marker file at all is Absent, not Unreadable"
        );

        write_run_tombstone_marker(&maintenance, &run_id, &tombstone, 1_700_000_000_000)
            .await
            .unwrap();

        let state = read_run_tombstone_marker(&maintenance, &run_id).await;
        assert_eq!(
            state,
            TombstoneMarkerState::Present {
                run_id: run_id.clone(),
                tombstone_path: normalize_tombstone_path(&tombstone),
            }
        );
        assert!(marker_matches(
            &state,
            &run_id,
            &normalize_tombstone_path(&tombstone)
        ));
        assert!(
            !marker_matches(
                &state,
                &run_id,
                &normalize_tombstone_path(&temp.path().join(".deleting-run-other"))
            ),
            "a marker naming a DIFFERENT tree must not match"
        );

        // The address is IndexSegment, not raw encodeURIComponent: a run id that is already a
        // safe component is its own segment, so the file name is predictable.
        let path = run_tombstone_marker_path(&maintenance, &run_id);
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "0123456789abcdef0123456789abcdef.json"
        );

        // Corrupting the VERSION is Unreadable, never Absent — the distinction the self-heal and
        // the reap both depend on.
        let bytes = tokio::fs::read(&path).await.unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["version"] = serde_json::json!(99);
        tokio::fs::write(&path, serde_json::to_vec(&value).unwrap())
            .await
            .unwrap();
        assert_eq!(
            read_run_tombstone_marker(&maintenance, &run_id).await,
            TombstoneMarkerState::Unreadable
        );

        // So is a marker whose runId disagrees with the address it was found at.
        value["version"] = serde_json::json!(1);
        value["runId"] = serde_json::json!("someone-elses-run");
        tokio::fs::write(&path, serde_json::to_vec(&value).unwrap())
            .await
            .unwrap();
        assert_eq!(
            read_run_tombstone_marker(&maintenance, &run_id).await,
            TombstoneMarkerState::Unreadable
        );

        // And an empty tombstonePath (pi `:236`'s `!marker.tombstonePath`).
        value["runId"] = serde_json::json!(run_id.as_str());
        value["tombstonePath"] = serde_json::json!("");
        tokio::fs::write(&path, serde_json::to_vec(&value).unwrap())
            .await
            .unwrap();
        assert_eq!(
            read_run_tombstone_marker(&maintenance, &run_id).await,
            TombstoneMarkerState::Unreadable
        );
    }

    #[tokio::test]
    async fn a_marker_pointing_at_a_vanished_tree_self_heals() {
        let temp = tempfile::tempdir().unwrap();
        let maintenance = temp.path().join(".async-retention");
        let run_id = run("aaaa");
        let tombstone = temp.path().join(".deleting-run-gone");

        write_run_tombstone_marker(&maintenance, &run_id, &tombstone, 10)
            .await
            .unwrap();
        let marker_path = run_tombstone_marker_path(&maintenance, &run_id);
        assert!(marker_path.exists());

        assert_eq!(
            read_run_tombstone_marker_self_healing(&maintenance, &run_id).await,
            TombstoneMarkerState::Absent,
            "a marker whose tree is gone does not block"
        );
        assert!(
            !marker_path.exists(),
            "and it is unlinked, which is what stops markers accumulating forever"
        );
    }

    #[tokio::test]
    async fn the_self_heal_keeps_a_marker_whose_tree_still_exists_and_never_unlinks_an_unreadable_one()
     {
        let temp = tempfile::tempdir().unwrap();
        let maintenance = temp.path().join(".async-retention");
        let run_id = run("bbbb");
        let tombstone = temp.path().join(".deleting-run-live");
        tokio::fs::create_dir_all(&tombstone).await.unwrap();

        write_run_tombstone_marker(&maintenance, &run_id, &tombstone, 10)
            .await
            .unwrap();
        let marker_path = run_tombstone_marker_path(&maintenance, &run_id);
        assert!(matches!(
            read_run_tombstone_marker_self_healing(&maintenance, &run_id).await,
            TombstoneMarkerState::Present { .. }
        ));
        assert!(marker_path.exists());

        // An UNREADABLE marker is blocking and must never be unlinked — the self-heal only ever
        // removes a marker it could fully read and prove dangling.
        tokio::fs::write(&marker_path, b"{not json").await.unwrap();
        assert_eq!(
            read_run_tombstone_marker_self_healing(&maintenance, &run_id).await,
            TombstoneMarkerState::Unreadable
        );
        assert!(marker_path.exists());
    }

    /// pi `:231` — the `lstat` gate. A marker that is not a plain file cannot be trusted to BE a
    /// marker, and the answer must be `Unreadable` (blocking) rather than `Absent`: `Absent` would
    /// let the reap proceed on a tree nothing can attribute.
    #[tokio::test]
    async fn a_marker_that_is_not_a_regular_file_is_unreadable_never_absent() {
        let temp = tempfile::tempdir().unwrap();
        let maintenance = temp.path().join(".async-retention");
        let directory_run = run("dddd");
        let symlink_run = run("eeee");

        // A DIRECTORY where the marker belongs.
        let directory_marker = run_tombstone_marker_path(&maintenance, &directory_run);
        tokio::fs::create_dir_all(&directory_marker).await.unwrap();
        assert_eq!(
            read_run_tombstone_marker(&maintenance, &directory_run).await,
            TombstoneMarkerState::Unreadable
        );

        // A SYMLINK, even one pointing at a perfectly valid marker: `lstat` reports the link, and
        // following it would let anything outside the maintenance root decide what a tombstone
        // marker says.
        let real = temp.path().join("real-marker.json");
        tokio::fs::write(
            &real,
            serde_json::to_vec(&RunTombstoneMarker {
                version: TombstoneMarkerVersion,
                run_id: symlink_run.clone(),
                tombstone_path: temp.path().join(".deleting-run-x"),
                created_at: 1,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        let symlink_marker = run_tombstone_marker_path(&maintenance, &symlink_run);
        tokio::fs::create_dir_all(symlink_marker.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::symlink(&real, &symlink_marker).await.unwrap();
        assert_eq!(
            read_run_tombstone_marker(&maintenance, &symlink_run).await,
            TombstoneMarkerState::Unreadable,
            "the CONTENT is valid; the link is what disqualifies it"
        );

        // And neither is ever unlinked by the self-heal.
        assert_eq!(
            read_run_tombstone_marker_self_healing(&maintenance, &symlink_run).await,
            TombstoneMarkerState::Unreadable
        );
        assert!(tokio::fs::symlink_metadata(&symlink_marker).await.is_ok());
        assert!(directory_marker.exists());
    }

    #[tokio::test]
    async fn the_marker_primitives_survive_a_failed_rename_without_leaking() {
        // SCOPE OF THIS TEST, stated honestly because an earlier name overclaimed it. The rollback
        // INVARIANT lives in `sweep::retire_run` (`sweep.rs:575-585`): marker down, rename, and on
        // a rename error `remove_run_tombstone_marker` + `mutated = false`. `retire_run` is private
        // to `sweep`, so nothing in this module can drive it, and a test here that hand-rolls the
        // same three calls proves only that write-then-remove removes — which is trivially true.
        //
        // What this test legitimately pins is the PRIMITIVES that rollback is built from, against a
        // rename that genuinely cannot succeed (its target parent does not exist): the marker is on
        // disk before the rename is attempted, removing it afterwards leaves nothing behind, and the
        // run tree is untouched by the failure. That is real, and it is less than the old name
        // claimed.
        //
        // `retire_run`'s own error arm IS now covered, by
        // `sweep::tests::a_failed_rename_rolls_the_tombstone_marker_back_and_leaves_the_tree`. The
        // earlier note here said it could not be forced without an unwritable `async_root`
        // (useless under a root test runtime) or an injection seam; that was wrong. The same
        // "target parent does not exist" trick this test uses works there too — point `async_root`
        // at a directory that is gone and the rename fails with `ENOENT` while the source tree
        // stays real — and `retire_run` is private to `sweep`, whose own test module can drive it.
        let temp = tempfile::tempdir().unwrap();
        let async_root = temp.path().join("async");
        let maintenance = async_root.join(".async-retention");
        let run_id = run("cccc");
        let run_dir = async_root.join("cccc");
        tokio::fs::create_dir_all(&run_dir).await.unwrap();
        let tombstone = async_root.join("missing-parent").join(".deleting-run-x");

        write_run_tombstone_marker(&maintenance, &run_id, &tombstone, 10)
            .await
            .unwrap();
        assert!(
            run_tombstone_marker_path(&maintenance, &run_id).exists(),
            "the marker is on disk BEFORE the rename is attempted"
        );

        let rename = tokio::fs::rename(&run_dir, &tombstone).await;
        assert!(rename.is_err(), "the rename cannot succeed");
        remove_run_tombstone_marker(&maintenance, &run_id).await;

        assert!(
            !run_tombstone_marker_path(&maintenance, &run_id).exists(),
            "rollback leaves no marker"
        );
        assert!(run_dir.exists(), "and the run tree is untouched");
    }
}

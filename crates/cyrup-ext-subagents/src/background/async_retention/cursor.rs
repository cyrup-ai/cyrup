//! The persisted scan cursor — pi `RetentionCursor` (`:31-40`), `readCursor` (`:318-321`) and the
//! commit at `:906-907`.
//!
//! # Without this file the reaper silently does nothing, forever, on exactly the root it is for
//!
//! [`scan_run_candidates`](super::scan_run_candidates) returns at most
//! [`ASYNC_RETENTION_BATCH_SIZE`](super::ASYNC_RETENTION_BATCH_SIZE) candidates, taken as the
//! smallest names in byte order strictly greater than the cursor. A root whose first 100 entries
//! are all `recent` yields those same 100 every pass if the cursor is not persisted — entry 101 is
//! never evaluated, and the pass reports a clean `skipped: {recent: 100}` while the root grows
//! without bound. The bounded window and the persisted cursor are ONE mechanism; either alone is
//! useless.
//!
//! # The cursor is committed only by a pass that finished cleanly
//!
//! `:897-900`. The cursor's meaning is *"everything at or below this name has been handled"*. A
//! pass that renamed a tree onto a tombstone and then failed has NOT handled it, so it records
//! `commit-failure` and leaves the cursor where it was; the next pass re-sees the tombstone
//! instead of stranding it until the cursor wrapped all the way round. Same for a pass that lost
//! its lock mid-flight (`:902-905`).

use std::path::{Path, PathBuf};

/// pi `CURSOR_NAME` (`:21`) — `<maintenance_root>/.async-retention-cursor.json`.
pub const CURSOR_NAME: &str = ".async-retention-cursor.json";

/// pi `RetentionCursor` (`:31-40`), narrowed to the ONE key the run half needs.
///
/// # [CYRUP-DELTA] the six result-side keys are absent, not defaulted
///
/// `resultAfter`, `resultPublicAfter`, `resultPendingAfterBySession`, `resultReplayAfter`,
/// `resultArchiveAfter` and `pendingSessionAfter` address `resultsDir`, which
/// [`super`]'s boundary hands to [`crate::background::result_index`] and
/// [`crate::background::completion_replay`] — neither of which is cursor-paginated, because
/// neither is budget-bounded. Carrying dead keys through a record this module writes would invite
/// a later reader to believe they mean something. `runAfter` is the only cursor key the run half
/// has ever needed (`worker.mjs:57-68`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionCursor {
    /// pi `version: 1`. A record from a future build FAILS to parse, and [`read_cursor`] then
    /// starts from the beginning of the name order — which re-scans, never skips.
    pub version: CursorVersion,
    /// pi `runAfter` — the last async-root entry name the previous pass returned. `None` means
    /// "start at the smallest name", which is both the initial state and the state after a wrap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_after: Option<String>,
}

/// The cursor's on-disk version, as a TYPE — [`super::TombstoneMarkerVersion`]'s shape.
///
/// Failing closed here means "re-scan from the start", which is the SAFE direction for a cursor:
/// the worst outcome is that names already handled are evaluated again and skipped, whereas a
/// half-read cursor could advance past entries that were never evaluated at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorVersion;

/// The only version this build reads or writes (`:320`).
const CURSOR_VERSION: u32 = 1;

impl serde::Serialize for CursorVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(CURSOR_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for CursorVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == CURSOR_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported retention-cursor version {raw} (this build reads version \
                 {CURSOR_VERSION})"
            )))
        }
    }
}

/// `<maintenance_root>/.async-retention-cursor.json`.
#[must_use]
pub fn cursor_path(maintenance_root: &Path) -> PathBuf {
    maintenance_root.join(CURSOR_NAME)
}

/// pi `readCursor` (`:318-321`) — absent, unreadable or wrong-version all read as the default.
pub async fn read_cursor(maintenance_root: &Path) -> RetentionCursor {
    let Ok(bytes) = tokio::fs::read(cursor_path(maintenance_root)).await else {
        return RetentionCursor::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// pi `writeAtomicJson(path.join(maintenanceRoot, CURSOR_NAME), cursor)` (`:907`).
///
/// Atomic because two instances in a shared scope can both hold it open across a break-and-retake
/// of the lock, and a torn cursor would send the next pass to an arbitrary point in the name
/// order.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when the maintenance root cannot be made accessible
/// or the atomic write fails. The sweep records it and declines to claim the pass advanced.
pub async fn write_cursor(
    maintenance_root: &Path,
    cursor: &RetentionCursor,
) -> std::io::Result<()> {
    crate::background::ensure_accessible_dir(maintenance_root).await?;
    crate::background::atomic::write_atomic_json(&cursor_path(maintenance_root), cursor).await
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

    #[tokio::test]
    async fn an_absent_cursor_reads_as_the_start_of_the_name_order() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_cursor(temp.path()).await, RetentionCursor::default());
        assert!(read_cursor(temp.path()).await.run_after.is_none());
    }

    #[tokio::test]
    async fn a_written_cursor_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let cursor = RetentionCursor {
            version: CursorVersion,
            run_after: Some("run-zzz".to_string()),
        };
        write_cursor(temp.path(), &cursor).await.unwrap();
        assert_eq!(read_cursor(temp.path()).await, cursor);
    }

    #[tokio::test]
    async fn a_future_version_cursor_restarts_the_scan_rather_than_skipping() {
        let temp = tempfile::tempdir().unwrap();
        tokio::fs::write(
            cursor_path(temp.path()),
            br#"{"version":9,"runAfter":"run-zzz"}"#.as_slice(),
        )
        .await
        .unwrap();
        assert_eq!(
            read_cursor(temp.path()).await.run_after,
            None,
            "an unreadable cursor must re-scan from the start, never advance past unseen entries"
        );
    }

    #[tokio::test]
    async fn a_cleared_cursor_serialises_without_the_key() {
        let temp = tempfile::tempdir().unwrap();
        write_cursor(temp.path(), &RetentionCursor::default())
            .await
            .unwrap();
        let text = tokio::fs::read_to_string(cursor_path(temp.path()))
            .await
            .unwrap();
        assert!(text.contains("\"version\""));
        assert!(
            !text.contains("runAfter"),
            "pi `delete cursor.runAfter` (`worker.mjs:66`) is an absent key, not a null one"
        );
    }
}

//! The index retention sweep.
//!
//! Ports pi `cleanupResultIndexes` (`result-files.ts:473-518`).

use std::path::Path;

use super::entry::ResultIndexEntry;
use super::errno;
use super::exists;
use super::locate;
use super::paths;

/// The default retention window: 24 hours.
///
/// pi `cleanupResultIndexes`'s `maxAgeMs` default (`result-files.ts:473`).
pub const DEFAULT_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;

/// Remove index entries that no longer point at anything and are older than `max_age_ms`.
///
/// pi `cleanupResultIndexes` (`result-files.ts:473-518`). Returns the number of entries removed.
///
/// # What this does and does not sweep
///
/// Only `<results_dir>/result-index/`. Payloads are **never** removed here — a payload is deleted
/// exactly once, by the watcher, after it has been delivered
/// (`ResultsWatcher::consume`). Reaping orphaned run directories under the async root
/// is a separate concern, ported by `background/retention/`.
///
/// # The three-part removal condition
///
/// An entry is removed when it is unparseable, **or** when all three hold: no public payload, no
/// staged payload, and the entry file's own mtime is older than the cutoff. The age check is what
/// makes this safe to run concurrently with a live write: a result staged one second ago has no
/// public payload yet, and deleting its index would orphan it permanently.
///
/// Directories are visited depth-first and then `rmdir`'d; a non-empty or concurrently-modified
/// directory simply stays, which is why the removal is attempted rather than checked first.
///
/// # Errors
///
/// Propagates a genuine I/O fault from a directory listing. Missing, unreadable and
/// permission-denied directories are skipped — this is a best-effort background sweep.
pub async fn cleanup_result_indexes(
    results_dir: &Path,
    now_ms: i64,
    max_age_ms: i64,
) -> std::io::Result<usize> {
    let root = paths::result_index_root(results_dir);
    let cutoff = now_ms.saturating_sub(max_age_ms);
    let mut removed = 0usize;
    // pi recurses; an explicit stack keeps this a plain `async fn` rather than requiring a boxed
    // recursive future, and the traversal order is identical (children before the parent rmdir).
    visit(results_dir, &root, cutoff, &mut removed).await?;
    Ok(removed)
}

async fn visit(
    results_dir: &Path,
    dir: &Path,
    cutoff: i64,
    removed: &mut usize,
) -> std::io::Result<()> {
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(error) if errno::is_ignorable_listing_error(&error) => return Ok(()),
        Err(error) => return Err(error),
    };

    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        match entry.file_type().await {
            Ok(file_type) if file_type.is_dir() => subdirs.push(path),
            Ok(file_type)
                if file_type.is_file()
                    && path.extension().and_then(std::ffi::OsStr::to_str) == Some("json") =>
            {
                files.push(path);
            }
            _ => {}
        }
    }

    for subdir in subdirs {
        Box::pin(visit(results_dir, &subdir, cutoff, removed)).await?;
        // pi `:490-493`: attempt the rmdir and ignore failure — a directory that is non-empty or
        // being concurrently written is left in place rather than probed first.
        let _ = tokio::fs::remove_dir(&subdir).await;
    }

    for path in files {
        if should_remove(results_dir, &path, cutoff).await
            && tokio::fs::remove_file(&path).await.is_ok()
        {
            *removed += 1;
        }
    }
    Ok(())
}

/// pi `:497-517`'s per-file decision.
async fn should_remove(results_dir: &Path, path: &Path, cutoff: i64) -> bool {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            // pi `:509-516`: an unreadable index file is removed, not retried forever.
            return !errno::is_unaddressable(&error) && !errno::is_absent(&error);
        }
    };
    let Some(entry) = ResultIndexEntry::parse(&bytes) else {
        // pi `:503`: `!index` removes unconditionally, with no age check — an entry that cannot be
        // parsed can never become useful, so waiting 24 hours to delete it buys nothing.
        return true;
    };

    let has_public = locate::result_payload_location_from_index(results_dir, &entry)
        .await
        .is_some();
    if has_public {
        return false;
    }
    let has_pending = exists::first_existing(&paths::result_pending_paths(
        results_dir,
        &entry.session_id,
        &entry.run_id,
    ))
    .await
    .is_some();
    if has_pending {
        return false;
    }

    mtime_ms(&metadata).is_some_and(|mtime| mtime <= cutoff)
}

fn mtime_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
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
    use crate::background::RunId;
    use crate::background::result_index::write::{
        ResultWrite, write_async_result_file, write_pending_async_result_file,
    };
    use crate::identity::SessionId;

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        run_id: String,
        session_id: String,
    }

    fn payload() -> Payload {
        Payload {
            run_id: "run1".to_string(),
            session_id: "s1".to_string(),
        }
    }

    fn now_ms() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        )
        .expect("fits")
    }

    #[tokio::test]
    async fn sweeping_an_empty_tree_removes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            cleanup_result_indexes(tmp.path(), now_ms(), DEFAULT_MAX_AGE_MS)
                .await
                .expect("sweep"),
            0
        );
    }

    #[tokio::test]
    async fn a_live_index_with_a_public_payload_survives() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");

        // Even with an enormous "now", a live entry is kept: the payload still exists.
        let removed = cleanup_result_indexes(
            tmp.path(),
            now_ms() + DEFAULT_MAX_AGE_MS * 10,
            DEFAULT_MAX_AGE_MS,
        )
        .await
        .expect("sweep");
        assert_eq!(removed, 0);
        assert!(paths::result_index_path(tmp.path(), &session("s1"), &run).is_file());
    }

    #[tokio::test]
    async fn a_staged_payload_protects_its_index_regardless_of_age() {
        // The safety property: a just-staged result has no public payload, and deleting its index
        // would orphan it permanently.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let staged = paths::result_pending_path(tmp.path(), &session("s1"), &run);
        tokio::fs::create_dir_all(staged.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(&staged, br#"{"runId":"run1","sessionId":"s1"}"#)
            .await
            .expect("seed");
        // Index it by hand so the entry exists without the payload being public.
        super::super::write::write_result_index_for_data(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        })
        .await
        .expect("index");

        let removed = cleanup_result_indexes(
            tmp.path(),
            now_ms() + DEFAULT_MAX_AGE_MS * 10,
            DEFAULT_MAX_AGE_MS,
        )
        .await
        .expect("sweep");
        assert_eq!(removed, 0, "a staged payload must protect its index");
    }

    #[tokio::test]
    async fn an_orphaned_index_older_than_the_cutoff_is_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");
        // Delete the payload: the index now points at nothing.
        tokio::fs::remove_file(paths::result_owned_path(tmp.path(), &session("s1"), &run))
            .await
            .expect("rm");

        let removed = cleanup_result_indexes(
            tmp.path(),
            now_ms() + DEFAULT_MAX_AGE_MS * 10,
            DEFAULT_MAX_AGE_MS,
        )
        .await
        .expect("sweep");
        assert!(
            removed >= 1,
            "expected the orphaned entries to be swept, removed={removed}"
        );
        assert!(!paths::result_index_path(tmp.path(), &session("s1"), &run).exists());
    }

    #[tokio::test]
    async fn a_recent_orphan_is_kept_until_the_window_elapses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");
        tokio::fs::remove_file(paths::result_owned_path(tmp.path(), &session("s1"), &run))
            .await
            .expect("rm");

        // "now" is the present, so the just-written index is well inside the window.
        let removed = cleanup_result_indexes(tmp.path(), now_ms(), DEFAULT_MAX_AGE_MS)
            .await
            .expect("sweep");
        assert_eq!(
            removed, 0,
            "a fresh orphan must survive; a concurrent write may still be mid-flight"
        );
    }

    #[tokio::test]
    async fn an_unparseable_entry_is_removed_without_waiting_for_the_window() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = paths::session_index_dir(tmp.path(), &session("s1"));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let junk = dir.join("junk.json");
        tokio::fs::write(&junk, b"not json").await.expect("seed");

        let removed = cleanup_result_indexes(tmp.path(), now_ms(), DEFAULT_MAX_AGE_MS)
            .await
            .expect("sweep");
        assert_eq!(removed, 1);
        assert!(!junk.exists());
    }

    #[tokio::test]
    async fn payloads_are_never_removed_by_the_index_sweep() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");

        let _ = cleanup_result_indexes(
            tmp.path(),
            now_ms() + DEFAULT_MAX_AGE_MS * 10,
            DEFAULT_MAX_AGE_MS,
        )
        .await
        .expect("sweep");
        assert!(
            paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file(),
            "the sweep must never touch a payload"
        );
    }

    #[tokio::test]
    async fn emptied_directories_are_rmdird() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = paths::session_index_dir(tmp.path(), &session("s1"));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        tokio::fs::write(dir.join("junk.json"), b"nope")
            .await
            .expect("seed");

        let _ = cleanup_result_indexes(tmp.path(), now_ms(), DEFAULT_MAX_AGE_MS)
            .await
            .expect("sweep");
        assert!(
            !dir.exists(),
            "an emptied partition directory should not linger"
        );
    }

    #[tokio::test]
    async fn a_pending_write_survives_the_sweep_and_is_promoted_by_it() {
        // The sweep reads through `result_payload_location_from_index`, which promotes (pi:
        // `cleanupResultIndexes` -> `indexedResultFile` -> `resultPayloadLocationFromIndex`).
        // So a staged result is not merely spared — the sweep publishes it. That is the
        // reader-promotes property doing exactly its job: a payload staged by a runner that died
        // before promoting gets published by whoever next walks the tree.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &session("s1"),
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(),
        )
        .await
        .expect("write");

        let removed = cleanup_result_indexes(
            tmp.path(),
            now_ms() + DEFAULT_MAX_AGE_MS * 10,
            DEFAULT_MAX_AGE_MS,
        )
        .await
        .expect("sweep");
        assert_eq!(removed, 0, "a live result's index must never be swept");
        assert!(
            paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file(),
            "the sweep promoted the staged payload rather than stranding it"
        );
        assert!(
            paths::result_index_path(tmp.path(), &session("s1"), &run).is_file(),
            "and its index survives"
        );
    }
}

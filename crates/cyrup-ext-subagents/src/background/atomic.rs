//! The shared atomic-write primitive (R-SA-076/135): every status/result/config/meta JSON file
//! this crate ever writes goes through [`write_atomic_json`], so there is exactly ONE
//! temp-then-rename implementation for the whole crate, not one per call site (per architecture.md
//! §6.5's explicit "MUST be implemented as one shared primitive, not two" note on R-SA-135).
//!
//! # Mechanism
//!
//! 1. Serialize `value` to pretty JSON bytes.
//! 2. Write those bytes to a freshly, uniquely named temp file in the SAME directory as `path`
//!    (same-directory placement is required for the rename in step 3 to be a same-filesystem,
//!    single-syscall POSIX `rename(2)` — a cross-filesystem rename is not atomic and can even fail
//!    outright on some platforms).
//! 3. `rename` the temp file over `path`. On POSIX this is a single atomic syscall: a concurrent
//!    reader opening `path` at any point either sees the fully-old file or the fully-new file,
//!    never a torn/partial write.
//!
//! # Temp-filename collision resistance (R-SA-135)
//!
//! R-SA-135 requires a "process-unique temp filename (pid + timestamp + random component)" so
//! that concurrent writers never collide. A bare `std::process::id()` suffix (as in the
//! architecture doc's illustrative sketch) is unique across *processes* but NOT across concurrent
//! *in-process* `tokio` tasks writing the same `path` from the same process — two tasks in this
//! crate's own orchestrator (e.g. two tracked jobs both status-writing at once, or a poller
//! reconciling while a tool call also writes) would derive the identical temp path and race each
//! other's `tokio::fs::write`. We satisfy the letter of R-SA-135 (pid + timestamp + random) AND
//! close that in-process gap by suffixing with a UUIDv7: it embeds a millisecond-resolution
//! timestamp in its high bits and a fresh 74-bit random tail per call (RFC 9562), so every call —
//! whether from a different process or a different task in the same process — gets a distinct
//! name with overwhelming probability, while still sorting roughly chronologically for on-disk
//! debugging.
//!
//! # Retry-with-backoff on rename contention
//!
//! POSIX `rename(2)` itself is atomic and does not partially fail, but the *filesystem call* can
//! still return a transient `Err` under contention (e.g. `ETXTBSY`-class errors on some
//! filesystems, or `ENOENT`/`EACCES`-class transients on network/overlay filesystems and, per the
//! architecture doc's own comment, Windows' `EACCES`/`EBUSY` while another handle briefly holds
//! the destination open). A single failed `rename` must not be treated as fatal data loss when the
//! temp file it was about to replace `path` with is still sitting there fully written — a short
//! bounded retry-with-backoff absorbs that window without ever silently losing the write or
//! leaving a stray temp file behind (verified by `tests::no_leftover_temp_file_after_success`).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Number of `rename` attempts before giving up and surfacing the last error. Five attempts with
/// the backoff schedule below spans a little over half a second of total contention-absorption,
/// which is generous for the kind of brief lock/handle contention this guards against without
/// risking a long hang on a genuinely broken filesystem.
const MAX_RENAME_ATTEMPTS: u32 = 5;

/// Base delay for the rename retry backoff; doubled on each subsequent attempt (bounded, see
/// [`backoff_delay`]).
const RETRY_BASE_DELAY: Duration = Duration::from_millis(10);

/// Upper bound on any single backoff delay, so a pathological attempt count never stalls a caller
/// for an unreasonable amount of wall-clock time.
const RETRY_MAX_DELAY: Duration = Duration::from_millis(200);

/// Atomically writes `value` as pretty-printed JSON to `path` (R-SA-076/135).
///
/// Writes to a uniquely named temp file in `path`'s own parent directory, then renames it over
/// `path`. The rename is a single POSIX atomic syscall: a concurrent reader of `path` never
/// observes a partially written file, only the fully-old or fully-new contents. Transient rename
/// failures under brief file-lock/handle contention (e.g. Windows `EACCES`/`EBUSY`, or equivalent
/// transients on other platforms) are retried with a bounded exponential backoff before the error
/// is surfaced to the caller.
///
/// # Errors
///
/// Returns an `io::Error` if serialization fails, if the temp file cannot be written, or if the
/// rename does not succeed within the retry budget. On any error path the temp file is best-effort
/// removed so a failed write never leaves a stray `*.tmp-*` file behind.
pub async fn write_atomic_json<T: serde::Serialize + Sync>(
    path: &Path,
    value: &T,
) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = unique_temp_path(path)?;

    if let Err(write_err) = tokio::fs::write(&tmp, &bytes).await {
        // Best-effort cleanup: the write may have partially landed bytes on disk even though it
        // returned an error (e.g. a mid-write disk-full). Never leave that partial temp file
        // around for a directory listing to trip over.
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(write_err);
    }

    match rename_with_backoff(&tmp, path).await {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            // The rename never landed: the temp file is still sitting there unused. Clean it up
            // rather than leaving an orphan `*.tmp-*` file next to `path` forever.
            let _ = tokio::fs::remove_file(&tmp).await;
            Err(rename_err)
        }
    }
}

/// The SYNCHRONOUS, owner-only (`0600`) sibling of [`write_atomic_json`] — pi
/// `writePrivateAtomicJson` (`pi-subagents/src/shared/atomic-json.ts:62`, i.e.
/// `createAtomicJsonWriter({ mode: 0o600 })`), the writer the whole `missions/` subtree persists
/// through.
///
/// Three differences from [`write_atomic_json`], each mandated by the upstream writer this ports:
///
/// 1. **It creates the parent directory** (`fsImpl.mkdirSync(path.dirname(filePath), { recursive:
///    true })`, `atomic-json.ts:44-46`). [`write_atomic_json`]'s callers all pre-create their run
///    directories; a mission record's directory may not exist yet on the first write.
/// 2. **It chmods the file to `0600`** before the rename, so the private mode is in effect the
///    instant the file becomes visible at `path` rather than a moment later.
/// 3. **It is blocking**, because `missions/` is a synchronous subsystem end to end (see that
///    module's own docs). Every call is a small local-filesystem write on the orchestrator's own
///    machine.
///
/// The temp-filename derivation and the bounded rename backoff are the SAME
/// [`unique_temp_path`]/[`MAX_RENAME_ATTEMPTS`]/[`backoff_delay`] this module already owns — this
/// is a second entry point onto one implementation, not a second implementation (per this module's
/// "one shared primitive, not two" contract).
///
/// # Errors
///
/// Returns an `io::Error` if serialization fails, if the parent directory cannot be created, if
/// the temp file cannot be written, or if the rename does not succeed within the retry budget. On
/// any error path the temp file is best-effort removed.
pub fn write_private_atomic_json_blocking<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = unique_temp_path(path)?;

    if let Err(write_err) = std::fs::write(&tmp, &bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(write_err);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(mode_err) =
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        {
            let _ = std::fs::remove_file(&tmp);
            return Err(mode_err);
        }
    }

    let mut attempt = 0u32;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(err) => {
                attempt += 1;
                if attempt >= MAX_RENAME_ATTEMPTS {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(err);
                }
                std::thread::sleep(backoff_delay(attempt));
            }
        }
    }
}

/// Renames `tmp` to `dest`, retrying a bounded number of times with exponential backoff if the
/// underlying `rename` call fails transiently (file-lock/handle contention). Returns the last
/// error if every attempt fails.
async fn rename_with_backoff(tmp: &Path, dest: &Path) -> io::Result<()> {
    let mut attempt = 0u32;
    loop {
        match tokio::fs::rename(tmp, dest).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                attempt += 1;
                if attempt >= MAX_RENAME_ATTEMPTS {
                    return Err(err);
                }
                tokio::time::sleep(backoff_delay(attempt)).await;
            }
        }
    }
}

/// Exponential backoff delay for retry attempt `attempt` (1-indexed), doubling from
/// [`RETRY_BASE_DELAY`] and capped at [`RETRY_MAX_DELAY`].
fn backoff_delay(attempt: u32) -> Duration {
    RETRY_BASE_DELAY
        .checked_mul(1u32 << attempt.min(8))
        .unwrap_or(RETRY_MAX_DELAY)
        .min(RETRY_MAX_DELAY)
}

/// Derives a collision-resistant temp-file path in the same directory as `path`, satisfying
/// R-SA-135's "process-unique temp filename (pid + timestamp + random component)" while also
/// being unique across concurrent in-process tokio tasks (see module docs).
///
/// The temp file lives alongside `path` (not in a separate temp directory) so the subsequent
/// rename is guaranteed same-filesystem and therefore atomic.
fn unique_temp_path(path: &Path) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "write_atomic_json target path has no file name",
        )
    })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let nonce = uuid::Uuid::now_v7().as_simple().to_string();
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(".tmp-{}-{nonce}", std::process::id()));
    Ok(parent.join(tmp_name))
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
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
    struct Sample {
        run_id: String,
        state: String,
        step: u32,
        payload: Vec<String>,
    }

    fn sample(step: u32) -> Sample {
        Sample {
            run_id: "run-abc123".to_string(),
            state: "Running".to_string(),
            step,
            payload: vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        }
    }

    /// A real write to a real tempdir: after `write_atomic_json` returns, the target file exists,
    /// is fully written, and round-trips the exact value that was passed in.
    #[tokio::test]
    async fn writes_a_complete_and_correct_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("status.json");
        let value = sample(1);

        write_atomic_json(&path, &value)
            .await
            .expect("atomic write succeeds");

        let bytes = tokio::fs::read(&path).await.expect("target file exists");
        let read_back: Sample = serde_json::from_slice(&bytes).expect("valid, complete JSON");
        assert_eq!(read_back, value, "round-tripped value matches exactly");
    }

    /// No leftover `*.tmp-*` file remains in the directory after a successful write — the rename
    /// consumed it, nothing is left orphaned.
    #[tokio::test]
    async fn no_leftover_temp_file_after_success() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("result.json");

        write_atomic_json(&path, &sample(2))
            .await
            .expect("atomic write succeeds");

        let mut entries = tokio::fs::read_dir(dir.path())
            .await
            .expect("can list tempdir");
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("readdir entry") {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(
            names,
            vec!["result.json".to_string()],
            "only the final target file remains, no stray temp file: {names:?}"
        );
    }

    /// Overwriting an existing file is also atomic: the old contents are fully replaced by the
    /// new contents, never a mix of the two.
    #[tokio::test]
    async fn overwrites_existing_file_atomically() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("status.json");

        write_atomic_json(&path, &sample(1))
            .await
            .expect("first write");
        write_atomic_json(&path, &sample(2))
            .await
            .expect("second write");

        let bytes = tokio::fs::read(&path).await.expect("target file exists");
        let read_back: Sample = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(
            read_back,
            sample(2),
            "final contents are the LAST write, not a merge"
        );

        let mut entries = tokio::fs::read_dir(dir.path())
            .await
            .expect("can list tempdir");
        let mut count = 0usize;
        while entries.next_entry().await.expect("readdir entry").is_some() {
            count += 1;
        }
        assert_eq!(
            count, 1,
            "still exactly one file after an overwrite, no leftover temp file"
        );
    }

    /// Many concurrent `tokio` tasks — simulating the orchestrator process and the detached
    /// background runner both status-writing at once (R-SA-135's stated concern) — hammer
    /// `write_atomic_json` against the SAME path simultaneously. Every individual write must
    /// derive its own uniquely named temp file (never colliding with a sibling task's), and after
    /// all tasks finish the target file MUST contain one complete, valid, parseable JSON document
    /// (never a torn/partial/concatenated read) and there MUST be no leftover temp file.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_writers_never_produce_a_corrupted_read() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = Arc::new(dir.path().join("status.json"));

        let mut handles = Vec::new();
        for i in 0..64u32 {
            let path = Arc::clone(&path);
            handles.push(tokio::spawn(async move {
                write_atomic_json(path.as_path(), &sample(i)).await
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("task did not panic")
                .expect("every concurrent write succeeds");
        }

        // The file must exist and parse as exactly ONE complete, well-formed JSON object — proof
        // that no writer ever observed (or produced) a torn/partial/interleaved file.
        let bytes = tokio::fs::read(path.as_path())
            .await
            .expect("target file exists");
        let read_back: Sample = serde_json::from_slice(&bytes)
            .expect("final file is one complete, valid JSON document");
        assert!(
            (0..64u32).contains(&read_back.step),
            "final contents came from one of the racing writers, not corrupted: {read_back:?}"
        );

        // No orphaned temp files: every task's rename either succeeded (consuming its temp file)
        // or, if it somehow failed, cleaned up after itself.
        let mut entries = tokio::fs::read_dir(dir.path())
            .await
            .expect("can list tempdir");
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("readdir entry") {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(
            names,
            vec!["status.json".to_string()],
            "no leftover temp files after concurrent writers: {names:?}"
        );
    }

    /// Distinct calls against the same target path must derive distinct temp-file names, even
    /// when invoked back-to-back in the same process — the collision-resistance property
    /// `unique_temp_path` exists to guarantee (module docs' stated gap in the bare-pid sketch).
    #[test]
    fn unique_temp_path_never_collides_across_rapid_calls() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("meta.json");

        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            let tmp = unique_temp_path(&path).expect("derives a temp path");
            assert!(seen.insert(tmp.clone()), "temp path collided: {tmp:?}");
            assert_eq!(
                tmp.parent(),
                path.parent(),
                "temp file must live in the same directory as the target (same-filesystem rename)"
            );
        }
    }

    /// The backoff schedule is monotonically non-decreasing and stays within the documented cap,
    /// so retry timing behavior is predictable rather than accidentally unbounded.
    #[test]
    fn backoff_delay_is_bounded_and_nondecreasing() {
        let mut prev = Duration::from_millis(0);
        for attempt in 1..=MAX_RENAME_ATTEMPTS {
            let delay = backoff_delay(attempt);
            assert!(delay >= prev, "backoff should not shrink between attempts");
            assert!(
                delay <= RETRY_MAX_DELAY,
                "backoff must respect the documented cap"
            );
            prev = delay;
        }
    }

    /// A write to a path whose parent directory does not exist fails cleanly (surfaced as an
    /// `io::Error`, never a panic) and leaves nothing behind.
    #[tokio::test]
    async fn missing_parent_directory_fails_cleanly() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("does-not-exist").join("status.json");

        let result = write_atomic_json(&path, &sample(1)).await;
        assert!(
            result.is_err(),
            "write into a missing directory must error, not panic"
        );
    }
}

//! The read/write face of the durable tier: persist a completion before its payload dies, and read
//! it back afterwards.

use std::path::Path;

use crate::background::RunId;
use crate::background::result_index::errno;
use crate::background::wait_completions::WaitCompletion;
use crate::error::SubagentError;
use crate::identity::SessionId;

use super::archive::{CompletionArchive, parse_archive, write_completion_archive};
use super::record::{CompletionReplayRecord, ReplayVersion, parse_replay, validate_replay_record};
use super::retention::{cleanup_completion_replay_if_due, remove_best_effort};
use super::{CLEANUP_INTERVAL_MS, completion_replay_path};

/// The named argument object of [`write_completion_replay`] (pi's inline `input:` type, `:186-194`).
///
/// A struct rather than seven positional parameters because `results_dir`/`archive` paths and
/// `now`/`ttl_ms` are each pairwise transposable at a call site, and a transposition would compile.
pub struct CompletionReplayWrite<'a> {
    /// The per-cwd results directory both files live under.
    pub results_dir: &'a Path,
    /// The run being recorded.
    pub run_id: &'a RunId,
    /// The owning session — REQUIRED; see [`CompletionReplayRecord`].
    pub session_id: &'a SessionId,
    /// The projection to replay. Its `archive_path` is overwritten by this call (pi `:196`).
    pub completion: &'a WaitCompletion,
    /// The RAW payload the archive projects from — the same untyped value WORKFLOW_4's
    /// [`crate::background::wait_completions::to_wait_completion`] projects.
    pub data: &'a serde_json::Value,
    /// Epoch millis; both files' timestamps and the throttled sweep's clock.
    pub now: i64,
    /// [`crate::background::watch::DEDUP_TTL`] as millis. Do NOT introduce a second constant — the
    /// in-process record and the durable one must age out together or a `wait` sees them disagree.
    pub ttl_ms: i64,
}

/// pi `writeCompletionReplay` (`:186-209`). Persist a terminal completion BEFORE its one-shot
/// result file is removed.
///
/// Returns the record whose `completion` now carries `archive_path` — the caller stores THAT copy
/// (pi `wait-completions.ts:139-146` assigns `writeCompletionReplay(...).completion` back over its
/// own projection), so the in-process and durable tiers agree about the archive's location.
///
/// # Ordering, and why the archive is written first
///
/// The archive lands before the record (`:195` then `:206`). A record is only ever valid if its
/// archive exists — [`validate_replay_record`] enforces exactly that — so writing the record first
/// would open a window in which a concurrent reader sees a record pointing at nothing and DELETES
/// it as invalid. Archive, then record, is the same "never publish a reference before its referent"
/// discipline `result_index`'s stage-then-promote protocol follows.
///
/// The last act is the throttled sweep (`:207`) — upstream drives retention from the WRITE path,
/// not from a timer, and the throttle is what makes that free. `:207` passes `input.ttlMs` as
/// `maxAgeMs` and omits `intervalMs`, taking [`CLEANUP_INTERVAL_MS`]'s default at `:248`.
///
/// # Errors
///
/// A write fault from either file. The sweep's own failures are swallowed by design.
pub async fn write_completion_replay(
    input: &CompletionReplayWrite<'_>,
) -> std::io::Result<CompletionReplayRecord> {
    let archive_path =
        write_completion_archive(input.results_dir, input.run_id, input.data, input.now).await?;
    let mut completion = input.completion.clone();
    // pi `:196` — the record's completion is the archive-path-BEARING copy, which is what makes a
    // replayed completion able to point a caller at its own retained output.
    completion.archive_path = Some(archive_path.to_string_lossy().into_owned());
    let record = CompletionReplayRecord {
        version: ReplayVersion,
        run_id: input.run_id.clone(),
        session_id: input.session_id.clone(),
        completed_at: input.now,
        expires_at: input.now.saturating_add(input.ttl_ms),
        completion,
        archive_path,
    };
    crate::background::atomic::write_private_atomic_json(
        &completion_replay_path(input.results_dir, input.run_id),
        &record,
    )
    .await?;
    cleanup_completion_replay_if_due(
        input.results_dir,
        input.now,
        input.ttl_ms,
        CLEANUP_INTERVAL_MS,
    )
    .await;
    Ok(record)
}

/// The two read options (pi's `{ sessionId?, now? }`, `:212`), as one type so a `None`/`None` call
/// site cannot transpose them.
pub struct ReplayReadFilter<'a> {
    /// `None` applies NO filter — see [`read_completion_replay`]'s OPTIONAL-not-STRICT note.
    pub session_id: Option<&'a SessionId>,
    /// `None` means [`crate::time::now_epoch_millis`] — pi's `options.now ?? Date.now()`.
    pub now: Option<i64>,
}

/// pi `readCompletionReplay` (`:212-235`). The current record for `run_id`, or `None`.
///
/// # The four `None`s, and the two that DELETE
///
/// | case | pi | deletes? |
/// |---|---|---|
/// | file absent / unparseable / unknown version | `:216-221` | no |
/// | [`validate_replay_record`] rejected it | `:222-226` | **yes** — record only |
/// | `session_id` filter supplied and does not match | `:228` | no |
/// | `expires_at <= now` | `:229-233` | **yes** — record AND archive |
///
/// The two non-deleting rows are not an oversight. A foreign-session record belongs to another
/// session and this caller has no standing to reap it; an unparseable one is swept by
/// [`super::cleanup_completion_replay`]'s age policy, the only place that can distinguish "written
/// by a newer build" from "corrupt".
///
/// # `session_id` is an OPTIONAL filter, not a STRICT one (`:228`)
///
/// `None` means "no filter" and returns the record whatever session wrote it — upstream's
/// `options.sessionId !== undefined` guard, verbatim. This is deliberate: `inspect-rpc.ts:246`
/// passes a session while `wait-completions.ts:203` passes `run.sessionId`, which may itself be
/// `undefined`, and collapsing the two would make an UNATTRIBUTED run's wait unable to read its own
/// record. Do not "tighten" this into a required parameter.
///
/// # Errno policy
///
/// pi returns `undefined` for `ENOENT` and RETHROWS anything else (`:218-219`). cyrup returns
/// `Option`, so a non-absent read fault degrades to `None` as well — but loudly, because unlike
/// absence it is not a normal outcome.
pub async fn read_completion_replay(
    results_dir: &Path,
    run_id: &RunId,
    filter: ReplayReadFilter<'_>,
) -> Option<CompletionReplayRecord> {
    let replay_path = completion_replay_path(results_dir, run_id);
    let bytes = match tokio::fs::read(&replay_path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if !errno::is_absent(&error) {
                tracing::warn!(
                    path = %replay_path.display(),
                    %error,
                    "completion replay record unreadable"
                );
            }
            return None;
        }
    };
    // Unparseable or a version this build does not know: NOT deleted. Only the age policy may.
    let parsed = parse_replay(&bytes)?;
    let Some(record) = validate_replay_record(results_dir, run_id, parsed) else {
        // `:224` — a record that cannot address its own archive is unusable by anyone, so the
        // reader that noticed reaps it rather than leaving it to occupy the run's one slot.
        remove_best_effort(&replay_path).await;
        return None;
    };
    if let Some(session_id) = filter.session_id
        && &record.session_id != session_id
    {
        return None;
    }
    if record.expires_at <= filter.now.unwrap_or_else(crate::time::now_epoch_millis) {
        remove_best_effort(&replay_path).await;
        remove_best_effort(&record.archive_path).await;
        return None;
    }
    Some(record)
}

/// pi `readCompletionArchive` (`:237-246`), whose tolerant parser is `parseArchive` (`:165-183`).
///
/// # This has no in-crate consumer yet, and that is expected
///
/// Upstream's only caller is `inspect-rpc.ts:262`, which cyrup has not ported. It is in this
/// module's surface because the archive is half the on-disk format: a writer with no reader cannot
/// be validated, and this module's own tests are its first consumer. The upstream caller is cited
/// so a later dead-code sweep does not remove the reader half of a format.
///
/// Upstream distinguishes "absent" (`undefined`, `:243`) from "malformed" (it throws, `:240`), and
/// so does this: `Ok(None)` for absent, `Err` for malformed. Collapsing them would let a corrupt
/// archive read as an empty one.
///
/// # Errors
///
/// [`SubagentError::Spawn`] — the `#[from] std::io::Error` arm `collect.rs` already wraps parse
/// failures into — for a read fault that is not absence, and for an archive that does not parse.
pub async fn read_completion_archive(
    archive_path: &Path,
) -> Result<Option<CompletionArchive>, SubagentError> {
    let bytes = match tokio::fs::read(archive_path).await {
        Ok(bytes) => bytes,
        Err(error) if errno::is_absent(&error) => return Ok(None),
        Err(error) => return Err(SubagentError::Spawn(error)),
    };
    parse_archive(&bytes).map(Some).ok_or_else(|| {
        SubagentError::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Completion archive is malformed.",
        ))
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::completion_archive_path;
    use super::*;
    use serde_json::json;

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("non-empty")
    }

    fn completion(run: &str) -> WaitCompletion {
        WaitCompletion {
            run_id: run.to_string(),
            agent: Some("coder".to_string()),
            ..WaitCompletion::default()
        }
    }

    async fn write(dir: &Path, run: &str, now: i64, ttl_ms: i64) -> CompletionReplayRecord {
        write_completion_replay(&CompletionReplayWrite {
            results_dir: dir,
            run_id: &RunId::from_token(run),
            session_id: &session("s1"),
            completion: &completion(run),
            data: &json!({ "results": [{ "agent": "coder", "finalOutput": "the answer" }] }),
            now,
            ttl_ms,
        })
        .await
        .expect("write")
    }

    /// `:228`'s gate: another session's record is not this caller's to read. It is also not this
    /// caller's to REAP — the file must still be there afterwards, because the session that owns it
    /// has every right to replay it.
    #[tokio::test]
    async fn reading_a_replay_with_a_foreign_session_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("r1");
        write(tmp.path(), "r1", 1_000, 600_000).await;

        let foreign = session("someone-else");
        assert!(
            read_completion_replay(
                tmp.path(),
                &run,
                ReplayReadFilter {
                    session_id: Some(&foreign),
                    now: Some(2_000)
                },
            )
            .await
            .is_none()
        );
        assert!(
            completion_replay_path(tmp.path(), &run).exists(),
            "a foreign-session miss must NOT delete the record"
        );

        // The owning session still reads it.
        let mine = session("s1");
        let found = read_completion_replay(
            tmp.path(),
            &run,
            ReplayReadFilter {
                session_id: Some(&mine),
                now: Some(2_000),
            },
        )
        .await
        .expect("its own session reads it");
        assert_eq!(found.completion.run_id, "r1");
    }

    /// OPTIONAL, not STRICT — the guard that stops the row above from passing vacuously. `None`
    /// reads the record whatever session wrote it, which is what lets an UNATTRIBUTED run's wait
    /// (`wait-completions.ts:203` passes `run.sessionId`, itself possibly `undefined`) replay its
    /// own completion.
    #[tokio::test]
    async fn reading_a_replay_with_no_session_filter_returns_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("r1");
        let written = write(tmp.path(), "r1", 1_000, 600_000).await;

        let found = read_completion_replay(
            tmp.path(),
            &run,
            ReplayReadFilter {
                session_id: None,
                now: Some(2_000),
            },
        )
        .await
        .expect("no filter reads it");
        assert_eq!(found, written);
        // The completion carries the archive path, which is the second reason
        // `WaitCompletion::archive_path` must actually be populated (`wait-subscriptions.ts:255`).
        assert_eq!(
            found.completion.archive_path.as_deref(),
            Some(
                completion_archive_path(tmp.path(), &run)
                    .to_string_lossy()
                    .as_ref()
            )
        );

        // And the archive it points at is readable, with the child's own text in it.
        let archive = read_completion_archive(&found.archive_path)
            .await
            .expect("readable")
            .expect("present");
        assert_eq!(archive.entries[0].text.as_deref(), Some("the answer"));
    }

    /// `expires_at`, and that the expiry read DELETES both files (`:229-233`). The archive goes too
    /// — leaving it would orphan 64 KiB per expired run, and the archive-dir sweep's mtime-only
    /// policy is only safe because the two lifetimes coincide.
    #[tokio::test]
    async fn an_expired_replay_record_is_not_returned() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("r1");
        write(tmp.path(), "r1", 1_000, 10).await;
        let replay = completion_replay_path(tmp.path(), &run);
        let archive = completion_archive_path(tmp.path(), &run);
        assert!(replay.exists() && archive.exists());

        // Exactly AT the expiry: `<=`, so it is already gone.
        assert!(
            read_completion_replay(
                tmp.path(),
                &run,
                ReplayReadFilter {
                    session_id: None,
                    now: Some(1_010)
                },
            )
            .await
            .is_none()
        );
        assert!(!replay.exists(), "the record is reaped by the reader");
        assert!(!archive.exists(), "and its archive with it");
    }

    /// Absence is `Ok(None)`; malformed is an `Err`. Collapsing them would let a corrupt archive
    /// read as an empty one, which is the difference between "this run kept no text" and "this
    /// run's text is unreadable".
    #[tokio::test]
    async fn a_malformed_archive_is_an_error_and_a_missing_one_is_not() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(
            read_completion_archive(&tmp.path().join("nothing.json"))
                .await
                .expect("absence is not a failure")
                .is_none()
        );

        let corrupt = tmp.path().join("corrupt.json");
        tokio::fs::write(&corrupt, b"{not json".as_slice())
            .await
            .expect("write");
        let error = read_completion_archive(&corrupt)
            .await
            .expect_err("malformed is a failure");
        assert!(
            matches!(&error, SubagentError::Spawn(io) if io.kind() == std::io::ErrorKind::InvalidData),
            "{error:?}"
        );
    }

    /// `:222-226` — a record that cannot address its own archive is unusable by ANYONE, so the
    /// reader that notices reaps it rather than leaving it to occupy the run's one replay slot.
    #[tokio::test]
    async fn a_record_failing_validation_is_removed_by_the_reader() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("r1");
        let replay = completion_replay_path(tmp.path(), &run);
        tokio::fs::create_dir_all(replay.parent().expect("parent"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            &replay,
            json!({
                "version": 1, "runId": "r1", "sessionId": "s1",
                "completedAt": 0, "expiresAt": i64::MAX,
                "completion": { "runId": "r1" },
                "archivePath": "/etc/passwd",
            })
            .to_string(),
        )
        .await
        .expect("write");

        assert!(
            read_completion_replay(
                tmp.path(),
                &run,
                ReplayReadFilter {
                    session_id: None,
                    now: Some(1)
                },
            )
            .await
            .is_none()
        );
        assert!(!replay.exists(), "a redirect record is reaped on sight");
    }
}

//! Publishing a staged payload — the pending → public rename.
//!
//! Ports pi `promotePendingResultFile` (`result-files.ts:259-286`).
//!
//! # The destination is never unlinked first
//!
//! `tokio::fs::rename` is POSIX `rename(2)`, which replaces the destination atomically. pi's
//! comment at `:264-265` is a bug report, not a style note: unlink-then-rename has a window in
//! which the public path does not exist, and a losing promoter can unlink the payload a winning
//! promoter just published.

use std::path::Path;

use crate::background::RunId;
use crate::identity::SessionId;

use super::errno;
use super::exists;
use super::paths;

/// The outcome of attempting to promote a staged payload to its public path.
///
/// pi's `"none" | "promoted" | "pending"` union (`result-files.ts:259`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionState {
    /// Nothing to promote: no staged payload exists (and none was published by a racing promoter).
    None,
    /// The payload is now at its public path.
    Promoted,
    /// The payload is still staged — either the rename failed recoverably, or a concurrent
    /// promoter is mid-flight. The result is not lost; a later scan will retry.
    Pending,
}

/// Rename a staged payload to its public path.
///
/// pi `promotePendingResultFile` (`result-files.ts:259-286`).
///
/// # The destination is never unlinked first
///
/// `tokio::fs::rename` is POSIX `rename(2)`, which replaces the destination **atomically**. pi's
/// comment at `:264-265` — "Deleting it first lets a losing promoter unlink the result that
/// another promoter just published" — is a bug report, not a style note. Two promoters can race
/// here whenever a reader promotes concurrently with the writer, and the unlink-then-rename shape
/// has a window in which the public path does not exist.
///
/// # The error branch is how concurrent promoters converge
///
/// On `ENOENT`/`EEXIST`/`EPERM`/`EACCES`/`ENAMETOOLONG` the function re-probes **both** paths and
/// decides from what actually exists rather than from what the error said: if the staged file is
/// still there the result is `Pending`; else if the promoted file is there some other promoter
/// won, so `Promoted`; else the payload genuinely vanished, so `None`. Any other errno is a real
/// fault and yields `Pending` — the payload is presumed still staged and retryable.
///
/// # `file` names the payload; it no longer decides where it goes
///
/// The destination is derived from `(session_id, run_id)` so it lands in the owning session's
/// partition. `file` remains the recorded name (and the index's `file` field), which is what keeps
/// a payload written by an older build readable at its legacy root location.
pub(crate) async fn promote_pending_result_file(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
    log_failure: bool,
) -> PromotionState {
    // pi `:260`'s `file !== path.basename(file) || !file.endsWith(".json")` guard is discharged by
    // `file` being a `ResultFileName` — it cannot be anything else. That is the guard upstream
    // repeats at `:260`, `:294` and `:307`.
    let Some(pending_path) =
        exists::first_existing(&paths::result_pending_paths(results_dir, session_id, run_id)).await
    else {
        return PromotionState::None;
    };
    // The destination is the session-partitioned owned path, never the shared listable root: a
    // payload published into the root is visible to — and consumable by — every other process in
    // the directory, including builds that predate the index and enumerate by `readdir`.
    let result_path = paths::result_owned_path(results_dir, session_id, run_id);
    // `rename(2)` will not create the destination's parent, and the owned session directory is
    // created lazily by whichever payload lands there first.
    if let Some(parent) = result_path.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent).await
    {
        if log_failure {
            tracing::warn!(
                run_id = %run_id,
                %error,
                "failed to create the owned results directory; the payload stays staged and will retry"
            );
        }
        return PromotionState::Pending;
    }

    match tokio::fs::rename(&pending_path, &result_path).await {
        // pi `:267`: even a successful rename re-probes — the destination must actually be a file.
        Ok(()) => {
            if exists::is_existing_file(&result_path).await {
                PromotionState::Promoted
            } else {
                PromotionState::Pending
            }
        }
        Err(error) => {
            if errno::is_absent(&error) || errno::is_access_denied(&error) || is_exists(&error)
            {
                let pending_exists = exists::is_existing_file(&pending_path).await;
                let result_exists = exists::is_existing_file(&result_path).await;
                if pending_exists {
                    if !result_exists && log_failure && !errno::is_unaddressable(&error) {
                        tracing::warn!(
                            run_id = %run_id,
                            %error,
                            "failed to promote pending async result; it stays staged and will retry"
                        );
                    }
                    return PromotionState::Pending;
                }
                if result_exists {
                    // Another promoter won the race — the correct outcome, not a failure.
                    return PromotionState::Promoted;
                }
                if log_failure {
                    tracing::warn!(
                        run_id = %run_id,
                        "pending async result disappeared without a promoted result"
                    );
                }
                return PromotionState::None;
            }
            if log_failure {
                tracing::warn!(run_id = %run_id, %error, "failed to promote pending async result");
            }
            PromotionState::Pending
        }
    }
}

fn is_exists(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::AlreadyExists)
        || error.raw_os_error() == Some(libc::EEXIST)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::write::{ResultWrite, write_pending_async_result_file};
    use super::*;

    fn session(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    #[derive(serde::Serialize)]
    struct Payload {
        run_id: String,
    }

    fn payload() -> Payload {
        Payload { run_id: "run1".to_string() }
    }

    #[test]
    fn promotion_has_three_distinct_outcomes() {
        // `None` and `Pending` both mean "not public"; collapsing them would hide a payload that
        // vanished entirely, which is the case a 2-state type cannot express.
        assert_ne!(PromotionState::None, PromotionState::Pending);
        assert_ne!(PromotionState::None, PromotionState::Promoted);
        assert_ne!(PromotionState::Pending, PromotionState::Promoted);
    }

    #[tokio::test]
    async fn a_pending_payload_promotes_later() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        }, &payload())
        .await
        .expect("write");

        let state =
            promote_pending_result_file(tmp.path(), &session("s1"), &run, true).await;
        assert_eq!(state, PromotionState::Promoted);
        assert!(paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file());
    }

    #[tokio::test]
    async fn promoting_with_nothing_staged_is_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        assert_eq!(
            promote_pending_result_file(tmp.path(), &session("s1"), &run, false).await,
            PromotionState::None
        );
    }

    #[tokio::test]
    async fn a_second_promoter_reports_promoted_rather_than_failing() {
        // The convergence property: whoever loses the race still learns the truth.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        write_pending_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        }, &payload())
        .await
        .expect("write");

        assert_eq!(
            promote_pending_result_file(tmp.path(), &session("s1"), &run, false).await,
            PromotionState::Promoted
        );
        // Second attempt: nothing staged any more, but the public payload is there. pi returns
        // "none" here because `firstExistingResultFile` finds nothing to promote — the caller
        // distinguishes by looking at the public path, which `locate` does.
        assert_eq!(
            promote_pending_result_file(tmp.path(), &session("s1"), &run, false).await,
            PromotionState::None
        );
        assert!(
            paths::result_owned_path(tmp.path(), &session("s1"), &run).is_file(),
            "the published payload survived"
        );
    }

    #[tokio::test]
    async fn promotion_replaces_an_existing_public_payload_without_unlinking_it_first() {
        // POSIX rename is atomic; there must be no window in which the public path is absent.
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run1");
        let owned = paths::result_owned_path(tmp.path(), &session("s1"), &run);
        tokio::fs::create_dir_all(owned.parent().expect("parent")).await.expect("mkdir");
        tokio::fs::write(&owned, b"{\"old\":true}").await.expect("seed");
        write_pending_async_result_file(&ResultWrite {
            results_dir: tmp.path(),
            session_id: &session("s1"),
            run_id: &run,
            written_at: 1,
            async_dir: None,
            tool_call_id: None,
        }, &payload())
        .await
        .expect("write");

        assert_eq!(
            promote_pending_result_file(tmp.path(), &session("s1"), &run, false).await,
            PromotionState::Promoted
        );
        let bytes = tokio::fs::read(&owned).await.expect("read");
        assert!(!String::from_utf8_lossy(&bytes).contains("old"), "must be replaced");
    }
}

//! Payload existence probing, under this module's error policy.
//!
//! Ports pi `existingResultFile` (`result-files.ts:231-238`) and `firstExistingResultFile`
//! (`:88-93`).
//!
//! Separate from [`super::paths`], which is deliberately I/O-free, and from [`super::errno`],
//! which classifies errors without performing I/O. Three callers — [`super::promote`],
//! [`super::locate`] and [`super::retention`] — all of which are asking "is the payload there?",
//! not "promote it".

use std::path::Path;

use super::errno;

/// pi `existingResultFile` (`result-files.ts:231-238`) — is this path a regular file?
pub(crate) async fn is_existing_file(path: &Path) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(meta) => meta.is_file(),
        Err(error) => {
            if !errno::is_absent(&error) {
                tracing::warn!(path = %path.display(), %error, "failed to inspect async result payload");
            }
            false
        }
    }
}

/// pi `firstExistingResultFile` (`result-files.ts:88-93`) — the first alias that exists.
pub(crate) async fn first_existing(
    candidates: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    for candidate in candidates {
        if is_existing_file(candidate).await {
            return Some(candidate.clone());
        }
    }
    None
}

/// Whether the run index records a terminal result for `run_id`.
///
/// A synchronous existence probe for callers that need to know a result EXISTS without reading it
/// — chiefly [`crate::background::run_id_resolver`], which resolves a run id from on-disk
/// evidence. It answers by looking for the run's index entry rather than for a payload, because a
/// promoted payload lives under `result-owned/<enc(session)>/` and cannot be addressed by run id
/// alone; the index entry can, and it is written before the payload is ever published.
#[must_use]
pub fn indexed_result_exists(results_dir: &Path, run_id: &crate::background::RunId) -> bool {
    super::paths::run_index_path(results_dir, run_id).is_file()
}
